// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Fuzzylite",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Fuzzylite", targets: ["Fuzzylite"])
    ],
    targets: [
        // The C interface of the Rust core, exposed to Swift through a
        // module map.
        .target(
            name: "FuzzyliteCCore",
            path: "Sources/FuzzyliteCCore",
            linkerSettings: [
                .unsafeFlags([
                    "-L", "../core/target/release",
                    "-lfuzzylite_core",
                ])
            ]
        ),
        .executableTarget(
            name: "Fuzzylite",
            dependencies: ["FuzzyliteCCore"],
            path: "Sources/Fuzzylite"
        ),
    ]
)
