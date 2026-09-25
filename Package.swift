// swift-tools-version: 5.9
import PackageDescription

// This file is automatically updated by CI after each release.
// The URL and checksum below are updated to point to the latest .xcframework.zip.
let package = Package(
    name: "MinigrafKit",
    platforms: [
        .iOS(.v16),
    ],
    products: [
        .library(
            name: "MinigrafKit",
            targets: ["minigrafFFI", "MinigrafKit"]
        ),
    ],
    targets: [
        .binaryTarget(
            name: "minigrafFFI",
            // Updated by CI: release.yml
            url: "https://github.com/project-minigraf/minigraf-swift/releases/download/v2.0.1/MinigrafKit-v2.0.1.xcframework.zip",
            checksum: "77522f0a288f0b865e90c777694a6490c7de2aefd6c0456b70c115d82c0786ce"
        ),
        .target(
            name: "MinigrafKit",
            dependencies: [.target(name: "minigrafFFI")],
            path: "Sources/MinigrafKit"
        ),
    ]
)
