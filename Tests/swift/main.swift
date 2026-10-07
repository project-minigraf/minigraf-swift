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

try? FileManager.default.removeItem(at: dir)
if failures > 0 {
    print("\(failures) check(s) failed")
    exit(1)
}
print("all Swift checks passed")
