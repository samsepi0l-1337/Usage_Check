// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "UsageCheck",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "UsageCheck", targets: ["UsageCheckApp"]),
        .library(name: "UsageCheckCore", targets: ["UsageCheckCore"])
    ],
    targets: [
        .target(name: "UsageCheckCore"),
        .executableTarget(
            name: "UsageCheckApp",
            dependencies: ["UsageCheckCore"]
        ),
        .testTarget(
            name: "UsageCheckCoreTests",
            dependencies: ["UsageCheckCore"]
        )
    ]
)
