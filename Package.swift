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
            url: "https://github.com/project-minigraf/minigraf-swift/releases/download/v2.0.4/MinigrafKit-v2.0.4.xcframework.zip",
            checksum: "e781942f43e03321d7da58ee58817acb66a182ad85d85eb3d41275848b69ac54"
        ),
        .target(
            name: "MinigrafKit",
            dependencies: [.target(name: "minigrafFFI")],
            path: "Sources/MinigrafKit"
        ),
    ]
)
