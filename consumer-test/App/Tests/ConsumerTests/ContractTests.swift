// The consumer contract: in-memory and file-backed use, a reopen, and the
// error type.
import XCTest
import Consumer
import MinigrafKit

final class ContractTests: XCTestCase {
    private func rows(_ json: String) throws -> [[Any]] {
        let object = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        return try XCTUnwrap(object?["results"] as? [[Any]], "no results in \(json)")
    }

    func testInMemory() throws {
        let db = try MiniGrafDb.openInMemory()
        _ = try db.execute(datalog: #"(transact [[:alice :name "Alice"] [:alice :age 30]])"#)
        let r = try rows(db.execute(datalog: "(query [:find ?n ?a :where [?e :name ?n] [?e :age ?a]])"))
        XCTAssertEqual(r.count, 1)
        XCTAssertEqual(r.first?.first as? String, "Alice")
        XCTAssertEqual((r.first?.last as? NSNumber)?.intValue, 30)
    }

    func testFileBackedSurvivesReopen() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let path = dir.appendingPathComponent("consumer.graph").path

        do {
            let db = try openDatabase(path: path)
            _ = try db.execute(datalog: #"(transact [[:bob :name "Bob"]])"#)
            try db.checkpoint()
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: path), "no .graph file at \(path)")

        let reopened = try openDatabase(path: path)
        let r = try rows(reopened.execute(datalog: "(query [:find ?n :where [?e :name ?n]])"))
        XCTAssertEqual(r.first?.first as? String, "Bob", "file-backed database lost its data on reopen")
    }

    func testPreparedQuery() throws {
        let db = try MiniGrafDb.openInMemory()
        _ = try db.execute(datalog: #"(transact [[:alice :name "Alice"] [:bob :name "Bob"]])"#)
        let pq = try db.prepare(datalog: "(query [:find ?n :where [?e :name $name] [?e :name ?n]])")
        for name in ["Alice", "Bob"] {
            XCTAssertEqual(try rows(pq.execute(["name": name])).first?.first as? String, name)
        }
        let typed = try rows(pq.execute(["name": MiniGrafValue.text(value: "Alice")]))
        XCTAssertEqual(typed.first?.first as? String, "Alice", "a typed value binds")
    }

    func testSetValues() throws {
        let db = try MiniGrafDb.openInMemory()
        _ = try db.execute(datalog: #"(transact [[:alice :tags #{1 "x"}] [:alice :name "Alice"]])"#)
        let tags = try rows(db.execute(datalog: "(query [:find ?t :where [?e :tags ?t]])"))
        XCTAssertEqual((tags.first?.first as? [Any])?.count, 2, "a set value comes back as an array")
        let pq = try db.prepare(datalog: "(query [:find ?n :where [?e :tags ?t] [(in? 1 $s)] [?e :name ?n]])")
        XCTAssertEqual(try rows(pq.execute(["s": Set([1, 2])])).first?.first as? String, "Alice", "a set binds")
    }

    func testErrorType() throws {
        let db = try MiniGrafDb.openInMemory()
        XCTAssertThrowsError(try db.execute(datalog: "(nosuchcommand [])")) { error in
            XCTAssertTrue(error is MiniGrafError, "threw \(type(of: error)), not MiniGrafError")
            let text = String(describing: error)
            XCTAssertNotNil(text.range(of: #"\[[A-Z]{3}-\d{3}\]"#, options: .regularExpression),
                            "the error has no code: \(text)")
        }
    }
}
