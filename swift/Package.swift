// swift-tools-version: 6.0
import Foundation
import PackageDescription

// An absolute path, not a relative one: `swift build` resolves relative
// `.unsafeFlags` against the package root, but `xtool dev` does not preserve
// that assumption (fails with "search path ... not found" otherwise).
let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path

let package = Package(
    name: "Todo",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "TodoApp", targets: ["TodoApp"]),      // iOS entry (xtool)
        .executable(name: "TodoMac", targets: ["TodoMac"]),   // macOS entry
    ],
    targets: [
        .systemLibrary(name: "TodoFFI", path: "Sources/TodoFFI"),
        .target(
            name: "TodoKit",
            dependencies: ["TodoFFI"],
            linkerSettings: [
                .unsafeFlags(["-L\(packageDir)/Sources/TodoFFI/lib"]),
                .linkedLibrary("todo_ffi"),
            ]
        ),
        .target(name: "TodoUI", dependencies: ["TodoKit"]),
        .target(name: "TodoApp", dependencies: ["TodoUI"]),
        // TodoKit isn't just transitive here: TodoMacApp.swift constructs
        // TodoModel directly, and SwiftPM requires importing a module to be
        // a direct dependency of the target, not just of one of its deps.
        .executableTarget(name: "TodoMac", dependencies: ["TodoUI", "TodoKit"]),
    ]
)
