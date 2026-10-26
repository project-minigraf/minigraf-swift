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
            url: "https://github.com/project-minigraf/minigraf-swift/releases/download/v2.0.11/MinigrafKit-v2.0.11.xcframework.zip",
            checksum: "5e131e79a2f2d39370a3d523b2f6c2d151f28e0c81aca1f3514a2985af29329e"
        ),
        .target(
            name: "MinigrafKit",
            dependencies: [.target(name: "minigrafFFI")],
            path: "Sources/MinigrafKit"
        ),
    ]
)
