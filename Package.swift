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
            url: "https://github.com/project-minigraf/minigraf-swift/releases/download/1.2.1/MinigrafKit-1.2.1.xcframework.zip",
            checksum: "598d9e799f4010fd66b6732423fe6a82e10a0c7152279f70f065e80c4b0ba666"
        ),
        .target(
            name: "MinigrafKit",
            dependencies: [.target(name: "minigrafFFI")],
            path: "Sources/MinigrafKit"
        ),
    ]
)
