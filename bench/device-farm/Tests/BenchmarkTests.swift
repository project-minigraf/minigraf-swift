import Foundation
import UIKit
import XCTest

final class BenchmarkTests: XCTestCase {
    private let expected = (0..<1000).map(Int64.init)
    private let scalarQuery = "(query [:find ?n :where [?e :bench/n ?n]])"
    private let pointQuery = "(query [:find ?n :where [?e :bench/n $n] [?e :bench/n ?n]])"
    private lazy var transaction: String = {
        "(transact [" + (0..<1000).map { "[:bench/e\($0) :bench/n \($0)]" }.joined(separator: " ") + "])"
    }()
    private var timed = false
    private var doubleWork = false
    private var completed: [String] = []
    private var output: URL!

    private func emit(_ record: [String: Any]) throws {
        var data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
        try requireResult(data.count < 900, "log record too large")
        let line = String(decoding: data, as: UTF8.self)
        data.append(10)
        let handle = try FileHandle(forWritingTo: output)
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: data)
        NSLog("MinigrafBench: %@", line)
    }

    private func rows(_ json: String) throws -> [[Any]] {
        let object = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        guard let rows = object?["results"] as? [[Any]] else { throw BenchmarkFailure.invalid("missing results") }
        return rows
    }
    private func integers(_ rows: [[Any]]) throws -> [Int64] {
        try rows.map { row in
            try requireResult(row.count == 1, "wrong projection")
            guard let number = row[0] as? NSNumber else { throw BenchmarkFailure.invalid("noninteger result") }
            let kind = String(cString: number.objCType)
            try requireResult(["q", "i", "s", "l"].contains(kind), "noninteger result type")
            return number.int64Value
        }
    }
    private func result(_ json: String) throws -> [Int64] { try integers(rows(json)) }
    private func drain(_ cursor: MiniGrafCursor, _ size: Int32) throws -> [Int64] {
        defer { cursor.close() }
        var all: [Int64] = []
        while let json = try cursor.nextBatch(maxRows: size) {
            guard let rows = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [[Any]] else {
                throw BenchmarkFailure.invalid("bad cursor JSON")
            }
            try requireResult(!rows.isEmpty && rows.count <= Int(size), "bad cursor batch size")
            all += try integers(rows)
        }
        return all
    }
    private func records(_ db: MiniGrafDb) throws -> [FactRecord] {
        let log = try db.factLog(filter: FactFilter(attributes: [":bench/n"]))
        defer { log.close() }
        var all: [FactRecord] = []
        while let batch = try log.nextBatch(maxRecords: 256) {
            try requireResult(!batch.isEmpty && batch.count <= 256, "bad fact-log batch size")
            all += batch
        }
        return all
    }
    private func checkRecords(_ records: [FactRecord]) throws {
        let values = try records.map { record -> Int64 in
            guard case .int64(let value) = record.value else { throw BenchmarkFailure.invalid("bad fact value") }
            return value
        }
        try ResultChecks.numbers(values, expected)
        try requireResult(records.allSatisfy { $0.asserted && $0.attribute == ":bench/n" && $0.txCount == 1 }, "bad fact metadata")
        try requireResult(Set(records.map(\.entity)).count == 1000, "bad entity count")
    }
    final class Even: MiniGrafPredicate {
        func test(value: MiniGrafValue) throws -> Bool {
            guard case .int64(let value) = value else { throw BenchmarkFailure.invalid("bad predicate value") }
            return value % 2 == 0
        }
    }
    final class Sum: MiniGrafAggregate {
        final class Acc: MiniGrafAccumulator, @unchecked Sendable {
            var total: Int64 = 0
            func step(value: MiniGrafValue) throws {
                guard case .int64(let value) = value else { throw BenchmarkFailure.invalid("bad aggregate value") }
                total += value
            }
            func finish(count: Int64) throws -> MiniGrafValue {
                try requireResult(count == 1000, "bad aggregate count")
                return .int64(value: total)
            }
        }
        func accumulator() throws -> MiniGrafAccumulator { Acc() }
    }
    private func measure<S, T>(_ name: String, iterations: Int = 20,
                               setup: () throws -> S, action: (S) throws -> T,
                               validate: (T) throws -> Void,
                               cleanup: (S) throws -> Void = { _ in }) throws {
        let id = "binding/swift-ios/" + name
        func invoke() throws -> UInt64 {
            try autoreleasepool {
                if doubleWork {
                    return try QualificationWork.invoke(setup: setup, action: action, validate: validate,
                                                        cleanup: cleanup, clock: { DispatchTime.now().uptimeNanoseconds })
                }
                let state = try setup()
                var failure: Error?
                var elapsed: UInt64 = 0
                do {
                    let start = timed ? DispatchTime.now().uptimeNanoseconds : 0
                    let value = try action(state)
                    elapsed = timed ? DispatchTime.now().uptimeNanoseconds - start : 0
                    try validate(value)
                } catch { failure = error }
                do { try cleanup(state) } catch { if failure == nil { failure = error } }
                if let failure { throw failure }
                return elapsed
            }
        }
        if timed {
            for _ in 0..<3 { _ = try invoke() }
            for sample in 0..<10 {
                var total: UInt64 = 0
                for _ in 0..<iterations { total += try invoke() }
                try emit(["type": "sample", "id": id, "sample": sample, "iterations": iterations,
                          "total_ns": total, "ns_per_op": Double(total) / Double(iterations),
                          "thermal_state": ProcessInfo.processInfo.thermalState.rawValue])
            }
        } else { _ = try invoke() }
        completed.append(id)
        try emit(["type": "checked", "id": id])
    }
    private func memory<T>(_ name: String, action: () throws -> T, validate: (T) throws -> Void) throws {
        try measure(name, setup: { () }, action: { _ in try action() }, validate: validate)
    }
    private func seeded() throws -> MiniGrafDb {
        let db = try MiniGrafDb.openInMemory()
        _ = try db.execute(datalog: transaction)
        return db
    }
    private func seedFile(_ path: String) throws {
        let db = try MiniGrafDb.open(path: path)
        _ = try db.execute(datalog: transaction)
        try db.checkpoint()
    }
    private final class FileState {
        let path: String
        var db: MiniGrafDb?
        init(_ path: String) throws { self.path = path; db = try MiniGrafDb.open(path: path) }
    }

    func testOracleRejectsEmptyAndWrongRows() throws {
        XCTAssertThrowsError(try ResultChecks.numbers([], [0, 1, 2, 3]))
        XCTAssertThrowsError(try ResultChecks.numbers([0, 0, 3, 3], [0, 1, 2, 3]))
        try ResultChecks.numbers([3, 1, 0, 2], [0, 1, 2, 3])
    }
    func testSmoke() throws { try run(timed: false) }
    func testBenchmark() throws {
        let platform = try PlatformIdentity()
        try requireResult(platform.platform == "physical",
                          "#839 release timing requires a physical Device Farm device; use testQualificationBaseline or testQualificationSlowdown for simulator qualification")
        try run(timed: true)
    }
    func testQualificationBaseline() throws { try run(timed: true, qualification: true) }
    func testQualificationSlowdown() throws {
        doubleWork = true
        defer { doubleWork = false }
        try run(timed: true, qualification: true)
    }

    private func run(timed: Bool, qualification: Bool = false) throws {
        try requireResult(!doubleWork || (timed && qualification), "slowdown is qualification-only")
        self.timed = timed
        completed = []
        let mode = qualification ? "qualification" : (timed ? "benchmark" : "smoke")
        let variant = doubleWork ? "slowdown" : "baseline"
        let runID = UUID().uuidString
        let platform = try PlatformIdentity()
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let suffix = qualification ? "qualification-\(variant)" : mode
        output = documents.appendingPathComponent("minigraf-bench-\(suffix).jsonl")
        try Data().write(to: output)
        let metadataURL = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "build", withExtension: "json"))
        let metadata = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: metadataURL)) as? [String: Any])
        // Select the archive actually linked for this compile-time/runtime platform.
        let artifacts = try XCTUnwrap(metadata["native_artifacts"] as? [String: [String: Any]])
        let artifact = try XCTUnwrap(artifacts[platform.platform])
        let nativeHash = try XCTUnwrap(artifact["sha256"] as? String)
        let keys = qualification
            ? ["core_revision", "binding_revision", "harness_revision", "harness_sha256"]
            : ["core_revision", "binding_revision", "harness_sha256"]
        var identity = metadata.filter { keys.contains($0.key) }
        try requireResult(identity.count == keys.count, "incomplete build identity")
        identity["native_sha256"] = nativeHash
        var environment: [String: Any] = ["type": "environment", "mode": mode, "build": identity,
            "platform": platform.platform, "model": platform.machine, "os": platform.os,
            "thermal_state": ProcessInfo.processInfo.thermalState.rawValue,
            "storage": "app-documents", "rss_verified": false]
        if let model = platform.simulatorModel {
            environment["simulator_model_identifier"] = model
            for key in ["simulator_device_type", "simulator_runtime"] {
                environment[key] = metadata[key]
            }
        }
        if qualification {
            environment["variant"] = variant
            environment["double_work"] = doubleWork
            environment["run_id"] = runID
            environment["profile"] = try XCTUnwrap(metadata["profile"] as? String)
        }
        try emit(environment)
        if qualification { try emit(platform.runtimeRecord(metadata: metadata)) }
        let directory = documents.appendingPathComponent("bench-839-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let db = try seeded()
        _ = try db.execute(datalog: "(transact [[:bench/set :bench/tags #{1 2}]])")
        let even = Even(), sum = Sum()
        try db.registerPredicate(name: "bench-even?", predicate: even)
        try db.registerAggregate(name: "bench-sum", aggregate: sum)
        let point = try db.prepare(datalog: pointQuery)
        try memory("prepared_execute/1k", action: { try self.result(point.execute(["n": Int64(42)])) },
                   validate: { try ResultChecks.numbers($0, [42]) })
        let scan = try db.prepare(datalog: scalarQuery)
        for batch in [Int32(1), Int32(256)] {
            try memory("cursor_batch_\(batch)/1k", action: { try self.drain(scan.query(bindings: [:]), batch) },
                       validate: { try ResultChecks.numbers($0, self.expected) })
        }
        let set = try db.prepare(datalog: "(query [:find ?s :where [?e :bench/tags $s] [?e :bench/tags ?s]])")
        try memory("set_roundtrip/2", action: {
            let rows = try self.rows(set.execute(["s": Set<Int64>([1, 2])]))
            try requireResult(rows.count == 1 && rows[0].count == 1, "bad set projection")
            guard let elements = rows[0][0] as? [Any] else { throw BenchmarkFailure.invalid("bad set result") }
            return try self.integers(elements.map { [$0] })
        }, validate: { try ResultChecks.numbers($0, [1, 2]) })
        let predicateQuery = "(query [:find ?n :where [?e :bench/n ?n] [(bench-even? ?n)]])"
        let aggregateQuery = "(query [:find (bench-sum ?n) :where [?e :bench/n ?n]])"
        try memory("predicate/1k", action: { try self.result(db.execute(datalog: predicateQuery)) },
                   validate: { try ResultChecks.numbers($0, self.expected.filter { $0 % 2 == 0 }) })
        try memory("aggregate/1k", action: { try self.result(db.execute(datalog: aggregateQuery)) },
                   validate: { try ResultChecks.numbers($0, [499500]) })
        try memory("prepare/point", action: { try db.prepare(datalog: self.pointQuery) },
                   validate: { try ResultChecks.numbers(self.result($0.execute(["n": Int64(42)])), [42]) })
        try measure("register_predicate/first", setup: { try self.seeded() }, action: { fresh in
            try fresh.registerPredicate(name: "bench-even?", predicate: even); return fresh
        }, validate: { try ResultChecks.numbers(self.result($0.execute(datalog: predicateQuery)), self.expected.filter { $0 % 2 == 0 }) })
        try measure("register_aggregate/first", setup: { try self.seeded() }, action: { fresh in
            try fresh.registerAggregate(name: "bench-sum", aggregate: sum); return fresh
        }, validate: { try ResultChecks.numbers(self.result($0.execute(datalog: aggregateQuery)), [499500]) })
        try memory("fact_log_batch_256/1k", action: { try self.records(db) }, validate: checkRecords)
        try memory("options_error/negative_cache", action: { () -> String in
            do {
                _ = try MiniGrafDb.openWithOptions(path: directory.appendingPathComponent("invalid.graph").path,
                                                   options: OpenOptions(pageCacheSize: -1))
                throw BenchmarkFailure.invalid("negative cache accepted")
            } catch MiniGrafError.Api(let message) { return String(message.prefix(9)) }
        }, validate: { try requireResult($0 == "[API-017]", "wrong error code") })
        let seed = directory.appendingPathComponent("seed.graph").path
        try seedFile(seed)
        if doubleWork {
            // The original file preparation is outside all iterations. Keep that boundary,
            // with two equivalent files and one independent read-only open on each.
            let secondSeed = directory.appendingPathComponent("seed-second.graph").path
            try seedFile(secondSeed)
            var useSecond = false
            try measure("options_readonly_open/1k", setup: {
                defer { useSecond.toggle() }
                return useSecond ? secondSeed : seed
            }, action: { path in
                try MiniGrafDb.openWithOptions(path: path, options: OpenOptions(readOnly: true, pageCacheSize: 64,
                                                                               queryMemoryLimit: 16 * 1024 * 1024))
            }, validate: { try ResultChecks.numbers(self.result($0.execute(datalog: self.scalarQuery)), self.expected) })
        } else {
            try memory("options_readonly_open/1k", action: {
                try MiniGrafDb.openWithOptions(path: seed, options: OpenOptions(readOnly: true, pageCacheSize: 64,
                                                                               queryMemoryLimit: 16 * 1024 * 1024))
            }, validate: { try ResultChecks.numbers(self.result($0.execute(datalog: self.scalarQuery)), self.expected) })
        }
        try measure("file_transact/1k", iterations: 1,
                    setup: { try FileState(directory.appendingPathComponent(UUID().uuidString + ".graph").path) },
                    action: { try $0.db!.execute(datalog: self.transaction) }, validate: { json in
            let object = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
            try requireResult(((object?["transacted"] as? NSNumber)?.int64Value ?? 0) > 0, "missing transaction result")
        }, cleanup: { state in
            state.db = nil
            let reopened = try MiniGrafDb.open(path: state.path)
            try ResultChecks.numbers(self.result(reopened.execute(datalog: self.scalarQuery)), self.expected)
        })
        let source = try records(db)
        try measure("log_writer_append_finish/1k", iterations: 1, setup: {
            let path = directory.appendingPathComponent(UUID().uuidString + ".graph").path
            return (path, try MiniGrafLogWriter.create(path: path, options: OpenOptions()))
        }, action: { state in try state.1.appendBatch(records: source); try state.1.finish() },
                    validate: { _ in }, cleanup: { state in
            state.1.close()
            let reopened = try MiniGrafDb.open(path: state.0)
            try requireResult(self.records(reopened).sorted { $0.entity < $1.entity } == source.sorted { $0.entity < $1.entity },
                              "fact-log round trip changed records")
            try ResultChecks.numbers(self.result(reopened.execute(datalog: self.scalarQuery)), self.expected)
        })
        try requireResult(completed.count == 14 && Set(completed).count == 14, "incomplete benchmark")
        if qualification {
            // A completion marker certifies teardown too. Legacy modes retain their path.
            try FileManager.default.removeItem(at: directory)
            try emit(["type": "complete", "mode": mode, "variant": variant, "platform": platform.platform,
                      "run_id": runID, "count": completed.count,
                      "thermal_state": ProcessInfo.processInfo.thermalState.rawValue])
        } else {
            try emit(["type": "complete", "mode": mode, "ids": completed, "count": completed.count,
                      "thermal_state": ProcessInfo.processInfo.thermalState.rawValue])
        }
    }
}
