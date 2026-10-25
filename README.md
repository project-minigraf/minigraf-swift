# minigraf-swift

Swift/iOS binding for [Minigraf](https://github.com/project-minigraf/minigraf) — zero-config,
single-file, embedded bi-temporal graph database with Datalog queries.

## Installation

### Swift Package Manager

In Xcode: File → Add Package Dependencies → enter this repo URL.

Or add to `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/project-minigraf/minigraf-swift", from: "2.0.8")
]
```

> Each semver tag `v<version>` points to a commit on the `swift-releases` branch. That commit
> holds the `Package.swift` with the release's xcframework URL and checksum, plus the generated
> Swift sources. The matching source commit on `main` is tagged `source-v<version>`.
> Releases up to v2.0.0 were tagged differently; see [Older releases](#older-releases) below.
> The `v2.0.1` tag was moved to its `swift-releases` commit after the fact, so it resolves normally
> but has no `source-v2.0.1` tag.

### Older releases

Before the tagging fix, the release workflow put the semver tag on the source commit, whose
`Package.swift` has a placeholder checksum and no generated sources. The tags `v2.0.0` and
earlier do not resolve. For those versions, the working manifest is on the `swift-v<version>`
tag, which SPM cannot resolve by version. Pin that tag's commit instead, for example:

```swift
// swift-v2.0.0
.package(url: "https://github.com/project-minigraf/minigraf-swift", revision: "1307a095c1725d2813abbbdcf17557311d517d24")
```

Use `git ls-remote --tags https://github.com/project-minigraf/minigraf-swift 'swift-v*'` to
find the commit for another version.

No release up to and including v2.0.8 compiles: its xcframework names the C module map
`minigraf_ffiFFI.modulemap` instead of `module.modulemap`, so the generated Swift cannot import
the FFI module ("cannot find type 'RustBuffer' in scope"). Use v2.0.9 or later.

Requires iOS 16+.

## Quick start

```swift
import MinigrafKit

let db = try MiniGrafDb.openInMemory()
let result = try db.execute(datalog: #"(transact [[:alice :name "Alice"]])"#)
print(result)  // {"transacted":1}
```

## Open options, cursors, the fact log and the log writer

```swift
import MinigrafKit

// Read-only: shared lock, nothing written; writes throw MiniGrafError (API-014).
let src = try MiniGrafDb.openWithOptions(path: oldPath, options: OpenOptions(readOnly: true, pageCacheSize: 4096))

// A cursor's answer is fixed when it opens. Each batch is a JSON array of rows,
// encoded like execute()'s "results".
let cursor = try src.query(datalog: "(query [:find ?n :where [?e :name ?n]])")
try cursor.forEachBatch(size: 1000) { batch in /* parse rows */ }

// Copy every fact version, keeping tx and valid-time bounds, into a new file.
let out = try MiniGrafLogWriter.create(path: newPath, options: OpenOptions())
try src.factLog(filter: FactFilter()).forEachRecord { record in
    if !record.attribute.hasPrefix(":secret/") { try out.append(record: record) }
}
try out.advanceTxCount(txCount: try src.currentTxCount())
try out.finish()   // out.close() instead abandons the build and leaves no file
```

`appendBatch(records:)` appends many records per call. A record's value is a
`MiniGrafValue` (`.text`, `.int64`, `.float64`, `.bool`, `.ref`, `.keyword`, `.null`);
`validTimeForever` is the `validTo` of a fact valid forever. `MiniGrafError.message`
starts with the error's code, such as `[API-015]`.

`OpenOptions(queryMemoryLimit: bytes)` caps the bytes one query may hold, counted per
entry by the operators that keep state (join build sides, negation and `or` tables,
distinct sets, aggregate groups, window rows, rule-derived facts). A query that would hold
more throws `MiniGrafError.Query` with QRY-012 and frees what it held. The default, and
`Int64.max`, is no limit. Rows a cursor returns are not counted, so read a large answer
with `query(datalog:)`.

## Prepared queries

`db.prepare(datalog:)` parses a query once; run it with its `$slots` bound (names
without the `$`). Each run reads the database as it is then, and a prepared query keeps
its database open until both are released.

```swift
let pq = try db.prepare(datalog: "(query [:find ?n :where [?e :age $age] [?e :name ?n]])")
let thirty = try pq.execute(["age": 30])
try pq.query(["age": 40]).forEachBatch { batch in /* JSON rows */ }
```

`Bool`, `Int`/`Int32`/`Int64`, `Double`/`Float`, `String`, `nil` (`Null`) and `UUID`
(an entity, which also fits value slots as a ref) bind as they are: they conform to
`MinigrafBindable`. For the other kinds pass a `MiniGrafValue` (`.keyword`, `.ref`, a
whole-number `.float64`) or a `MiniGrafBindValue` (`.entity`, `.txCount` or `.timestamp`
for `:as-of`, `.anyValidTime` for `:valid-at`). A missing slot throws
`MiniGrafError.Api` with `[API-022]`, a slot in an attribute position `[API-023]`, and a
value of the wrong kind `[API-024]`.

A `Set` binds a set value, for `in?` and the other set predicates or a set-valued
attribute: `pq.execute(["ids": Set([1, 2, 3])])`. Its elements conform to
`MinigrafSetElement`: the plain values above, with `UUID` as a ref, or `MiniGrafValue`;
mix types with `Set<MiniGrafValue>` (`[.keyword(value: ":role/admin"), .int64(value: 1)]`).
A set inside a set throws `MiniGrafError.Api` with `[API-021]`. Set values come back as
JSON arrays from `execute` and as `MiniGrafValue.set` in fact records.

`MiniGrafError` is `.Parse`, `.Query`, `.Storage`, `.Api` (a bad argument, a closed
object, a bind mistake) or `.Other`.

## User functions

A predicate conforms to `MiniGrafPredicate` (`test(value:) throws -> Bool`) and is
called in Datalog as `[(name ?v)]`. An aggregate conforms to `MiniGrafAggregate`; its
`accumulator()` returns a new `MiniGrafAccumulator` (`step(value:)`, `finish(count:)`)
for each group or window partition, and it is used as `(name ?v)` in `:find` or with
`:over`. Values arrive as `MiniGrafValue` (`.int64(value:)`, `.text(value:)`, …), and
`finish` returns one. The protocols are `AnyObject, Sendable`: implement them with a
`final class` whose stored properties are immutable, or mark it `@unchecked Sendable`
when it keeps state of its own, as an accumulator does.

```swift
final class Adult: MiniGrafPredicate {
    func test(value: MiniGrafValue) throws -> Bool {
        guard case .int64(let age) = value else { throw MiniGrafUdfError.Failed(msg: "age is not an integer") }
        return age >= 18
    }
}

final class Product: MiniGrafAggregate {
    final class Acc: MiniGrafAccumulator, @unchecked Sendable {
        var product: Int64 = 1
        func step(value: MiniGrafValue) throws {
            if case .int64(let n) = value { product *= n }
        }
        func finish(count: Int64) throws -> MiniGrafValue { .int64(value: product) } // count: values stepped
    }
    func accumulator() throws -> MiniGrafAccumulator { Acc() }
}

try db.registerPredicate(name: "adult?", predicate: Adult())
try db.registerAggregate(name: "product", aggregate: Product())
_ = try db.execute(datalog: "(query [:find ?n :where [?e :age ?a] [(adult? ?a)] [?e :name ?n]])")
_ = try db.execute(datalog: "(query [:find ?g (product ?a) :where [?e :group ?g] [?e :age ?a]])")
_ = try db.execute(datalog: "(query [:find ?a (product ?a :over (:order-by ?a)) :where [?e :age ?a]])")
```

- **Failures.** A callback that throws stops the query with `QRY-014`, raised as
  `MiniGrafError.Query`; it is never read as `false` or a missing value. Throw
  `MiniGrafUdfError.Failed(msg:)` to choose the message. Any other error also fails the
  call, with the first line of its `String(describing:)` as the message. A
  `MiniGrafValue.ref` from `finish` whose value is not a UUID is a failing call too.
- **Lifecycle.** The database keeps a strong reference to each registered object until
  the database and every cursor and prepared query opened from it are released. There
  is no unregister. Predicates and aggregates have separate names, and registering a
  name twice in either is an error (`INT-040`). A query sees the functions registered
  when it opened; a prepared query, those registered when each run starts.
- **Threading.** A callback runs on the thread that calls `execute` (of the database or
  a prepared query) or a cursor's `nextBatch`, including a prepared query's cursor.
  Queries on different threads may call the same object at once, so it must be
  thread-safe. One accumulator is used by one query on one thread at a time. Keep
  callbacks short: a query on the main thread blocks it for as long as they run.
- **Cost.** Each call crosses the FFI once: one foreign call per value.
- **Re-entry.** A callback may query the same database, open a cursor or register another
  function. Inside a write transaction, the calls the core refuses there still return
  `INT-001`.

## Building from source

Requires Rust stable toolchain with iOS targets and Xcode.

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --target aarch64-apple-ios --release
cargo build --target aarch64-apple-ios-sim --release
cargo run --bin uniffi-bindgen -- generate \
  --library target/aarch64-apple-ios/release/libminigraf_ffi.a \
  --language swift \
  --out-dir Sources/MinigrafKit/
```

## Cascade release

This repo receives a `core-release` repository_dispatch from the minigraf monorepo
cascade whenever a new version of the `minigraf` core crate is published. The release
workflow pins the new version, builds the xcframework for iOS device and simulator,
zips it, updates `Package.swift` on the `swift-releases` branch, tags that commit with the
semver tag SPM resolves, and creates a GitHub Release with the xcframework.

## License

MIT OR Apache-2.0
