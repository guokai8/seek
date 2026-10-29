// swift-tools-version: 6.0
import PackageDescription
import Foundation

// The Rust core is compiled into a static library. Its path depends on the
// build configuration, so it is resolved from the environment that
// `build.sh` sets up. A debug build looks in `target/debug`, a release
// build in `target/release`.
let coreTarget = ProcessInfo.processInfo.environment["CARGO_TARGET_DIR"]
    ?? "../core/target"
let coreLibrary = ProcessInfo.processInfo.environment["FUZZYLITE_CORE_DIR"]
    ?? "\(coreTarget)/debug"

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
                    "-L", coreLibrary,
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
