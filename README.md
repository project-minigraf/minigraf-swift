# minigraf-swift

Swift/iOS binding for [Minigraf](https://github.com/project-minigraf/minigraf) — zero-config,
single-file, embedded bi-temporal graph database with Datalog queries.

## Installation

### Swift Package Manager

In Xcode: File → Add Package Dependencies → enter this repo URL.

Or add to `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/project-minigraf/minigraf-swift", from: "2.0.1")
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
find the commit for another version. New projects should use 2.0.1 or later.

Requires iOS 16+.

## Quick start

```swift
import MinigrafKit

let db = try MiniGrafDb.openInMemory()
let result = try db.execute(datalog: #"(transact [[:alice :name "Alice"]])"#)
print(result)  // {"transacted":1}
```

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
