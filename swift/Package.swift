// swift-tools-version: 6.0
import Foundation
import PackageDescription

// An absolute path, not a relative one: `swift build` resolves relative
// `.unsafeFlags` against the package root, but `xtool dev` does not preserve
// that assumption (fails with "search path ... not found" otherwise).
let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path

let package = Package(
    name: "Remember",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "RememberApp", targets: ["RememberApp"]),      // iOS entry (xtool)
        .executable(name: "RememberMac", targets: ["RememberMac"]),   // macOS entry
    ],
    targets: [
        .systemLibrary(name: "RememberFFI", path: "Sources/RememberFFI"),
        .target(
            name: "RememberKit",
            dependencies: ["RememberFFI"],
            linkerSettings: [
                .unsafeFlags(["-L\(packageDir)/Sources/RememberFFI/lib"]),
                .linkedLibrary("remember_ffi"),
            ]
        ),
        .target(name: "RememberUI", dependencies: ["RememberKit"]),
        // RememberKit isn't just transitive here either (see the same note on
        // RememberMac below): RememberIOSApp.swift constructs RememberModel directly.
        .target(
            name: "RememberApp",
            dependencies: ["RememberUI", "RememberKit"],
            // The iOS Simulator reads an app's entitlements from these two
            // sections, not from its code signature (which `xtool dev
            // --simulator` doesn't make) -- without them, every Keychain
            // call fails with -34018 and signing in is impossible. Same
            // thing Xcode does for simulator builds. A device build ignores
            // them: there, the provisioning profile xtool signs with grants
            // the real entitlements. See swift/README.md#apple.
            linkerSettings: [
                .unsafeFlags([
                    "-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__entitlements",
                    "-Xlinker", "\(packageDir)/../apple/Simulator.entitlements",
                    "-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__ents_der",
                    "-Xlinker", "\(packageDir)/../apple/Simulator.entitlements.der",
                ], .when(platforms: [.iOS])),
            ]
        ),
        // RememberKit isn't just transitive here: AppDelegate.swift constructs
        // RememberModel directly, and SwiftPM requires importing a module to be
        // a direct dependency of the target, not just of one of its deps.
        // RememberMac no longer depends on RememberUI: the Spotlight-style panel is
        // its own Mac-only UI (CaptureView.swift), and TaskListView/
        // TaskDetailView remain in use only by RememberApp (iOS).
        .executableTarget(name: "RememberMac", dependencies: ["RememberKit"]),
    ]
)
