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
> Releases up to v2.0.1 were tagged differently; see
> [Older releases](#older-releases) below.

### Older releases

Before the tagging fix, the release workflow put the semver tag on the source commit, whose
`Package.swift` has a placeholder checksum and no generated sources. Those tags do not resolve.
For those versions, the working manifest is on the `swift-v<version>` tag instead, which SPM
cannot resolve by version. Pin the `swift-releases` branch or a specific revision instead:

```swift
.package(url: "https://github.com/project-minigraf/minigraf-swift", branch: "swift-releases")
```

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
