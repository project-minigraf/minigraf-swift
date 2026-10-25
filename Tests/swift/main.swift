// Run by CI (swift-test job): compiled together with the generated bindings and
// Sources/MinigrafKit into one module, linked against libminigraf_ffi.
import Foundation

var failures = 0
func check(_ condition: Bool, _ what: String, line: Int = #line) {
    if !condition {
        print("FAIL line \(line): \(what)")
        failures += 1
    }
}

func errorText(_ body: () throws -> Void) -> String {
    do {
        try body()
        return "no error"
    } catch let e as MiniGrafError {
        return e.message
    } catch {
        return "unexpected error"
    }
}

let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
func path(_ name: String) -> String { dir.appendingPathComponent(name).path }
let ref = "00000000-0000-4000-8000-000000000001"
let query = "(query [:find ?e ?n :where [?e :n ?n]])"

// Cursor (#462)
do {
    let db = try MiniGrafDb.openInMemory()
    for i in 0..<25 { _ = try db.execute(datalog: "(transact [[:e\(i) :n \(i)]])") }
    let all = try JSONSerialization.jsonObject(with: Data(try db.execute(datalog: query).utf8)) as! [String: Any]
    let expected = (all["results"] as! [[Any]]).count
    for size: Int32 in [1, 7, 1000] {
        let cursor = try db.query(datalog: query)
        check(cursor.vars() == ["?e", "?n"], "vars")
        var rows = 0
        try cursor.forEachBatch(size: size) { batch in
            let decoded = try JSONSerialization.jsonObject(with: Data(batch.utf8)) as! [[Any]]
            check(!decoded.isEmpty && decoded.count <= Int(size), "batch size")
            rows += decoded.count
        }
        check(rows == expected, "rows for batch size \(size)")
    }
    let fixed = try db.query(datalog: query)
    _ = try db.execute(datalog: "(transact [[:late :n 99]])")
    var fixedRows = 0
    try fixed.forEachBatch { fixedRows += (try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [[Any]]).count }
    check(fixedRows == 25, "cursor fixed at open")
    let closed = try db.query(datalog: query)
    closed.close()
    check(try closed.nextBatch(maxRows: 1) == nil, "closed cursor ends")
    check(errorText { _ = try db.query(datalog: "(transact [[:a :n 1]])") }.hasPrefix("[API-012]"), "API-012")
}

// Open options (#465)
do {
    let p = path("ro.graph")
    do {
        let db = try MiniGrafDb.open(path: p)
        _ = try db.execute(datalog: "(transact [[:a :n 1] [:b :n 2]])")
        try db.checkpoint()
    }
    let ro = try MiniGrafDb.openWithOptions(path: p, options: OpenOptions(readOnly: true, pageCacheSize: 16))
    let ro2 = try MiniGrafDb.openWithOptions(path: p, options: OpenOptions(readOnly: true))
    check(try ro.currentTxCount() == 1 && ro2.currentTxCount() == 1, "two read-only handles")
    check(errorText { _ = try ro.execute(datalog: "(transact [[:c :n 3]])") }.hasPrefix("[API-014]"), "API-014")
    check(errorText { _ = try MiniGrafDb.open(path: p) }.hasPrefix("[STG-02"), "writer refused")
    let missing = path("missing.graph")
    check(errorText { _ = try MiniGrafDb.openWithOptions(path: missing, options: OpenOptions(readOnly: true)) }.hasPrefix("[STG-042]"), "STG-042")
    check(!FileManager.default.fileExists(atPath: missing), "missing file not created")
}

// Fact log and log writer (#467)
do {
    let src = try MiniGrafDb.open(path: path("src.graph"))
    _ = try src.execute(datalog: #"(transact {:valid-from "2024-01-01" :valid-to "2025-01-01"} [[:alice :name "Alice"] [:alice :tag :t/admin]])"#)
    _ = try src.execute(datalog: "(transact [[:bob :friend #uuid \"\(ref)\"] [:bob :score 2.5]])")
    _ = try src.execute(datalog: #"(retract [[:alice :name "Alice"]])"#)

    func records(_ db: MiniGrafDb, _ filter: FactFilter = FactFilter()) throws -> [FactRecord] {
        var out: [FactRecord] = []
        try db.factLog(filter: filter).forEachRecord(batchSize: 2) { out.append($0) }
        return out
    }
    let recs = try records(src)
    check(recs.count == 5, "five records")
    check(recs.map(\.txCount) == recs.map(\.txCount).sorted(), "tx order")
    check(recs.first { $0.attribute == ":friend" }?.value == .ref(value: ref), "ref value")
    check(recs.first { $0.attribute == ":tag" }?.value == .keyword(value: ":t/admin"), "keyword value")
    check(recs.first { $0.attribute == ":score" }?.validTo == validTimeForever, "forever")
    check(try records(src, FactFilter(txFrom: 2, txTo: 2)).allSatisfy { $0.txCount == 2 }, "tx filter")
    check(errorText { _ = try src.factLog(filter: FactFilter(entities: ["alice"])) }.hasPrefix("[API-017]"), "API-017")

    let out = path("out.graph")
    let w = try MiniGrafLogWriter.create(path: out, options: OpenOptions())
    try w.append(record: recs[0])
    try w.appendBatch(records: Array(recs.dropFirst()))
    try w.advanceTxCount(txCount: try src.currentTxCount())
    check(try w.txCount() == 3, "writer tx_count")
    try w.finish()
    check(errorText { try w.finish() }.hasPrefix("[API-018]"), "API-018")
    let copy = try MiniGrafDb.openWithOptions(path: out, options: OpenOptions(readOnly: true))
    check(try copy.currentTxCount() == src.currentTxCount(), "copied tx count")
    check(try records(copy) == recs, "copied records")

    let hole = path("hole.graph")
    let h = try MiniGrafLogWriter.create(path: hole, options: OpenOptions())
    try h.appendBatch(records: recs.filter { $0.txCount != 2 })
    try h.advanceTxCount(txCount: 3)
    try h.finish()
    let hdb = try MiniGrafDb.openWithOptions(path: hole, options: OpenOptions(readOnly: true))
    check(Set(try records(hdb).map(\.txCount)) == [1, 3], "hole")

    let bad = path("bad.graph")
    let b = try MiniGrafLogWriter.create(path: bad, options: OpenOptions())
    try b.append(record: recs.last!)
    let order = errorText { try b.appendBatch(records: [recs.last!, recs.first!]) }
    check(order.hasPrefix("[API-015]") && order.hasSuffix("(batch index 1)"), "API-015 batch index")
    b.close()
    check(!b.isOpen(), "closed writer")
    check(!FileManager.default.fileExists(atPath: bad) && !FileManager.default.fileExists(atPath: bad + ".partial"), "abandoned build leaves no file")
    check(errorText { _ = try MiniGrafLogWriter.create(path: out, options: OpenOptions()) }.hasPrefix("[STG-043]"), "STG-043")
}

// #322: walCheckpointNever leaves the WAL when the handle closes.
do {
    let p = path("never.graph")
    do {
        let db = try MiniGrafDb.openWithOptions(path: p, options: OpenOptions(walCheckpointThreshold: walCheckpointNever))
        _ = try db.execute(datalog: "(transact [[:a :n 1]])")
    }
    check(FileManager.default.fileExists(atPath: p + ".wal"), "WAL kept on close")
}

// #549: a query that would hold more than queryMemoryLimit bytes throws QRY-012.
do {
    // A cartesian product holds one side in full.
    let product = "(query [:find ?a ?b :where [?a :n _] [?b :m _]])"
    let small = try MiniGrafDb.openWithOptions(path: path("small.graph"), options: OpenOptions(queryMemoryLimit: 64))
    _ = try small.execute(datalog: "(transact [[:a :n 1] [:b :m 2]])")
    var isQuery = false
    do { _ = try small.execute(datalog: product) } catch MiniGrafError.Query { isQuery = true }
    check(isQuery, "QRY-012 is a query error")
    check(errorText { _ = try small.execute(datalog: product) }.hasPrefix("[QRY-012]"), "QRY-012")
    let max = try MiniGrafDb.openWithOptions(path: path("max.graph"), options: OpenOptions(queryMemoryLimit: Int64.max))
    _ = try max.execute(datalog: "(transact [[:a :n 1] [:b :m 2]])")
    check(errorText { _ = try max.execute(datalog: product) } == "no error", "Int64.max is no limit")
}

// #181: prepared queries.
do {
    let db = try MiniGrafDb.openInMemory()
    _ = try db.execute(datalog: #"(transact [[:a :n 1] [:b :n 2] [:c :n 3] [:a :s "x"] [:a :f 2.5] [:a :w 2.0] [:a :t true] [:a :k :kw] [:a :z nil]])"#)
    func rows(_ json: String) -> [[Any]] {
        let object = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
        return object?["results"] as? [[Any]] ?? []
    }
    let pq = try db.prepare(datalog: "(query [:find ?x :where [?e :n $n] [?e :n ?x]])")
    for n in 1...3 {
        let r = rows(try pq.execute(["n": n]))
        check(r.count == 1 && (r[0][0] as? NSNumber)?.intValue == n, "prepared row for \(n)")
    }
    check(rows(try pq.execute(["n": 9])).isEmpty, "no row for 9")

    let plain: [(String, (any MinigrafBindable)?)] = [
        (":n", Int64(1)), (":n", Int32(1)), (":s", "x"), (":f", 2.5), (":f", Float(2.5)),
        (":w", 2.0), (":t", true), (":z", nil),
    ]
    for (attr, value) in plain {
        let q = try db.prepare(datalog: "(query [:find ?e :where [?e \(attr) $v]])")
        check(rows(try q.execute(["v": value])).count == 1, "plain value for \(attr)")
    }

    let entity = rows(try db.execute(datalog: #"(query [:find ?e :where [?e :s "x"]])"#))[0][0] as! String
    let uuid = UUID(uuidString: entity)!
    check(rows(try db.prepare(datalog: "(query [:find ?x :where [$e :n ?x]])").execute(["e": uuid])).count == 1, "UUID in an entity slot")
    _ = try db.execute(datalog: "(transact [[:b :friend #uuid \"\(entity)\"]])")
    check(rows(try db.prepare(datalog: "(query [:find ?x :where [?x :friend $f]])").execute(["f": uuid])).count == 1, "UUID in a value slot")

    let typed: [(String, any MinigrafBindable)] = [
        ("(query [:find ?e :where [?e :k $v]])", MiniGrafValue.keyword(value: ":kw")),
        ("(query [:find ?x :where [$v :n ?x]])", MiniGrafValue.ref(value: entity)),
        ("(query [:find ?x :as-of $v :where [?e :n ?x]])", MiniGrafBindValue.txCount(value: 1)),
        ("(query [:find ?x :as-of $v :where [?e :n ?x]])", MiniGrafBindValue.timestamp(value: Int64.max)),
        ("(query [:find ?x :valid-at $v :where [:a :n ?x]])", MiniGrafBindValue.anyValidTime),
    ]
    for (q, value) in typed {
        check(!rows(try db.prepare(datalog: q).execute(["v": value])).isEmpty, "typed value in \(q)")
    }

    var seen = 0
    try pq.query(["n": 2]).forEachBatch(size: 1) { batch in
        seen += ((try JSONSerialization.jsonObject(with: Data(batch.utf8))) as? [[Any]])?.count ?? 0
    }
    check(seen == 1, "prepared cursor")

    var isApi = false
    do { _ = try pq.execute([:]) } catch MiniGrafError.Api { isApi = true }
    check(isApi, "a missing slot is an API error")
    check(errorText { _ = try pq.execute([:]) }.hasPrefix("[API-022]"), "API-022")
    check(errorText { _ = try pq.execute(["n": MiniGrafBindValue.txCount(value: 1)]) }.hasPrefix("[API-024]"), "API-024")
    check(errorText { _ = try db.prepare(datalog: "(query [:find ?v :where [?e $a ?v]])") }.hasPrefix("[API-023]"), "API-023")
    check(errorText { _ = try pq.execute(["n": MiniGrafBindValue.txCount(value: -1)]) }.hasPrefix("[API-017]"), "API-017")

    // #316: sets.
    let member = try db.prepare(datalog: "(query [:find ?e :where [?e :n ?n] [(in? ?n $s)]])")
    check(rows(try member.execute(["s": Set([1, 3])])).count == 2, "a Swift Set binds")
    check(rows(try member.execute(["s": Set<Int64>()])).isEmpty, "an empty set binds")
    check(rows(try member.execute(["s": MiniGrafValue.set(value: [.int64(value: 2), .int64(value: 2)])])).count == 1, "a typed set binds")
    _ = try db.execute(datalog: #"(transact [[:a :tags #{1 :k "x"}]])"#)
    let equal = try db.prepare(datalog: "(query [:find ?e :where [?e :tags $s]])")
    let tags: Set<MiniGrafValue> = [.int64(value: 1), .text(value: "x"), .keyword(value: ":k")]
    check(rows(try equal.execute(["s": tags])).count == 1, "a set equals a stored set")
    check(rows(try equal.execute(["s": Set<MiniGrafValue>([.int64(value: 1), .text(value: "x"), .text(value: ":k")])])).isEmpty, "a string is no keyword")
    let out = rows(try db.execute(datalog: "(query [:find ?t :where [:a :tags ?t]])"))
    check((out.first?.first as? [Any])?.count == 3, "a set is a JSON array")
    var records: [FactRecord] = []
    try db.factLog(filter: FactFilter(attributes: [":tags"])).forEachRecord(batchSize: 10) { records.append($0) }
    check(records.first?.value == .set(value: [.text(value: "x"), .int64(value: 1), .keyword(value: ":k")]), "a fact record holds the set")
    let refs = try db.prepare(datalog: "(query [:find ?x :where [$e :n ?x] [(in? $e $s)]])")
    check(rows(try refs.execute(["e": uuid, "s": Set([uuid])])).count == 1, "a UUID element is a ref")
    check(errorText { _ = try member.execute(["s": Set<MiniGrafValue>([.set(value: [])])]) }.hasPrefix("[API-021]"), "API-021")
}

// #180: user predicates and aggregates written in Swift.
final class IsEven: MiniGrafPredicate {
    func test(value: MiniGrafValue) throws -> Bool {
        guard case .int64(let n) = value else { throw MiniGrafUdfError.Failed(msg: "not an integer") }
        return n % 2 == 0
    }
}

/// Sums integers; `step` throws on `failOn`. One accumulator is used by one
/// query on one thread at a time, so its state needs no lock.
final class Total: MiniGrafAggregate {
    let failOn: Int64?
    init(failOn: Int64? = nil) { self.failOn = failOn }

    final class Sum: MiniGrafAccumulator, @unchecked Sendable {
        let failOn: Int64?
        var sum: Int64 = 0
        init(failOn: Int64?) { self.failOn = failOn }
        func step(value: MiniGrafValue) throws {
            guard case .int64(let n) = value else { throw MiniGrafUdfError.Failed(msg: "not an integer") }
            if n == failOn { throw MiniGrafUdfError.Failed(msg: "refused \(n)") }
            sum += n
        }
        func finish(count: Int64) throws -> MiniGrafValue { .int64(value: sum) }
    }

    func accumulator() throws -> MiniGrafAccumulator { Sum(failOn: failOn) }
}

final class Throws: MiniGrafPredicate {
    struct Own: Error, CustomStringConvertible { var description: String { "own error" } }
    func test(value: MiniGrafValue) throws -> Bool { throw Own() }
}

final class BadRef: MiniGrafAggregate {
    final class Acc: MiniGrafAccumulator {
        func step(value: MiniGrafValue) throws {}
        func finish(count: Int64) throws -> MiniGrafValue { .ref(value: "nope") }
    }
    func accumulator() throws -> MiniGrafAccumulator { Acc() }
}

/// Answers by querying the database it is registered on.
final class InGroup2: MiniGrafPredicate {
    let db: MiniGrafDb
    init(_ db: MiniGrafDb) { self.db = db }
    func test(value: MiniGrafValue) throws -> Bool {
        guard case .int64(let n) = value else { return false }
        return !udfRows(try db.execute(datalog: "(query [:find ?e :where [?e :n \(n)] [?e :g 2]])")).isEmpty
    }
}

/// Each call waits until a call from another thread arrives.
final class Rendezvous: MiniGrafPredicate, @unchecked Sendable {
    let lock = NSCondition()
    var arrived = 0
    func test(value: MiniGrafValue) throws -> Bool {
        lock.lock()
        defer { lock.unlock() }
        arrived += 1
        lock.broadcast()
        let deadline = Date().addingTimeInterval(10)
        while arrived < 2 {
            if !lock.wait(until: deadline) { throw MiniGrafUdfError.Failed(msg: "the other query never arrived") }
        }
        return true
    }
}

func udfRows(_ json: String) -> [[Int]] {
    let object = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
    let rows = object?["results"] as? [[Any]] ?? []
    return rows.map { $0.map { ($0 as? NSNumber)?.intValue ?? Int.min } }.sorted { $0.lexicographicallyPrecedes($1) }
}

func queryError(_ body: () throws -> Void) -> String {
    do {
        try body()
        return "no error"
    } catch MiniGrafError.Query(let msg) {
        return msg
    } catch {
        return "not a query error: \(error)"
    }
}

func udfDb() throws -> MiniGrafDb {
    let db = try MiniGrafDb.openInMemory()
    _ = try db.execute(datalog: "(transact [[:a :n 1] [:b :n 2] [:c :n 3] [:d :n 4] [:a :g 1] [:b :g 1] [:c :g 2] [:d :g 2]])")
    return db
}

do {
    let evenQuery = "(query [:find ?n :where [?e :n ?n] [(even? ?n)]])"

    let db = try udfDb()
    try db.registerPredicate(name: "even?", predicate: IsEven())
    check(udfRows(try db.execute(datalog: evenQuery)) == [[2], [4]], "a predicate filters rows")

    try db.registerAggregate(name: "total", aggregate: Total())
    check(udfRows(try db.execute(datalog: "(query [:find (total ?n) :where [?e :n ?n]])")) == [[10]], "an aggregate in :find")
    // A running total per group: 1, 1+2 in group 1; 3, 3+4 in group 2.
    let over = "(query [:find ?n (total ?n :over (:partition-by ?g :order-by ?n)) :where [?e :n ?n] [?e :g ?g]])"
    check(udfRows(try db.execute(datalog: over)) == [[1, 1], [2, 3], [3, 3], [4, 7]], "an aggregate with :over")

    var found: [[Int]] = []
    try db.query(datalog: evenQuery).forEachBatch(size: 1) { batch in
        found += ((try JSONSerialization.jsonObject(with: Data(batch.utf8))) as? [[NSNumber]] ?? []).map { $0.map(\.intValue) }
    }
    check(found.sorted { $0.lexicographicallyPrecedes($1) } == [[2], [4]], "a cursor calls a predicate")

    check(errorText { try db.registerPredicate(name: "even?", predicate: IsEven()) }.hasPrefix("[INT-040]"), "a name registers once")

    _ = try db.execute(datalog: #"(transact [[:e :n "x"]])"#)
    let failed = queryError { _ = try db.execute(datalog: evenQuery) }
    check(failed.hasPrefix("[QRY-014]") && failed.contains("not an integer"), "MiniGrafUdfError is QRY-014 with its message")

    let own = try udfDb()
    try own.registerPredicate(name: "broken?", predicate: Throws())
    check(queryError { _ = try own.execute(datalog: "(query [:find ?n :where [?e :n ?n] [(broken? ?n)]])") }
        == "[QRY-014] user function `broken?` failed: own error", "any Swift error is QRY-014 with its description")

    let step = try udfDb()
    try step.registerAggregate(name: "total", aggregate: Total(failOn: 2))
    let stepped = queryError { _ = try step.execute(datalog: "(query [:find (total ?n) :where [?e :n ?n]])") }
    check(stepped.hasPrefix("[QRY-014]") && stepped.contains("refused 2"), "a failing step is QRY-014")
    let cursorFailed = queryError {
        try step.query(datalog: "(query [:find (total ?n) :where [?e :n ?n]])").forEachBatch(size: 10) { _ in }
    }
    check(cursorFailed.hasPrefix("[QRY-014]"), "a failure in a cursor is QRY-014")

    let bad = try udfDb()
    try bad.registerAggregate(name: "bad", aggregate: BadRef())
    check(queryError { _ = try bad.execute(datalog: "(query [:find (bad ?n) :where [?e :n ?n]])") }.hasPrefix("[QRY-014]"),
          "a non-UUID ref from finish is QRY-014")

    let reentry = try udfDb()
    try reentry.registerPredicate(name: "g2?", predicate: InGroup2(reentry))
    check(udfRows(try reentry.execute(datalog: "(query [:find ?n :where [?e :n ?n] [(g2? ?n)]])")) == [[3], [4]],
          "a predicate queries the same database")

    let pq = try reentry.prepare(datalog: "(query [:find (total ?n) :where [?e :g $g] [?e :n ?n]])")
    check(errorText { _ = try pq.execute(["g": 1]) } != "no error", "an unregistered aggregate fails")
    try reentry.registerAggregate(name: "total", aggregate: Total())
    check(udfRows(try pq.execute(["g": 1])) == [[3]], "a prepared query sees functions registered before each run")
    var prepared: [String] = []
    try pq.query(["g": 2]).forEachBatch(size: 10) { prepared.append($0) }
    check(prepared == ["[[7]]"], "a prepared cursor calls an aggregate")

    // Each call waits until a call from the other thread arrives: this only
    // finishes if neither query holds a binding lock while the core runs.
    let shared = try udfDb()
    try shared.registerPredicate(name: "meet?", predicate: Rendezvous())
    let group = DispatchGroup()
    let outcomesLock = NSLock()
    var outcomes: [String] = []
    for _ in 0..<2 {
        group.enter()
        Thread.detachNewThread {
            let outcome = queryError { _ = try shared.execute(datalog: "(query [:find ?n :where [:a :n ?n] [(meet? ?n)]])") }
            outcomesLock.lock()
            outcomes.append(outcome)
            outcomesLock.unlock()
            group.leave()
        }
    }
    group.wait()
    check(outcomes == ["no error", "no error"], "two queries are in one predicate at once")
}

try? FileManager.default.removeItem(at: dir)
if failures > 0 {
    print("\(failures) check(s) failed")
    exit(1)
}
print("all Swift checks passed")
