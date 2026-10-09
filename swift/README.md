# swift

A single SwiftPM package (`Remember`). No `.xcodeproj`, no `binaryTarget`, no
XCFramework. Swift is presentation-only: views read `model.snapshot` and call
`model.dispatch(...)`. Sorting, filtering, date formatting and reordering all
arrive pre-computed from `remember-core`. Platform differences (navigation
chrome, app lifecycle) are the only logic allowed here.

Build it through `cargo xtask` (see [`crates/xtask`](../crates/xtask/README.md)),
not `swift build` directly.

## Targets

- **`RememberFFI`** — a `systemLibrary` target wrapping the generated C
  header and the `remember-ffi` static lib. Generated; never edit by hand.
- **`RememberKit`** — the generated bindings (`remember_ffi.swift`, never
  edited by hand) plus hand-written glue:
  - `Model.swift` — `RememberModel`, bridging `remember_core::App` into an
    `@Observable` SwiftUI model.
  - `KeychainStore.swift` — logic-free Keychain wrapper for the sync session.
  - `DueColor.swift`, `ListColor.swift` — map core states to SwiftUI colors.
- **`RememberUI`** — SwiftUI task list and detail views, used by the iOS app.
- **`RememberMac`** — the macOS app: a menu-bar app with a Spotlight-style
  capture panel behind a global hotkey (`AppDelegate`, `SpotlightPanel`,
  `CaptureView`, `GlobalHotKey`), plus the Settings window.
- **`RememberApp`** — the iOS `@main` entry point, built and installed by
  [`xtool`](https://github.com/xtool-org/xtool) using `xtool.yml`.

## Apple

`apple/` holds `Info-macOS.plist`, `Info-iOS.plist` and `AppIcon.png`.

The bundle ID is `no.bendik.todo`, kept from the app's original name, as are
the data folder, database file and keychain service. Renaming them would
orphan existing data and sign existing installs out.

## If the build fails to link

The Rust staticlib needs whatever system libraries `rustc` linked against.
Never guess — ask it:

```bash
cargo rustc -p remember-ffi --release -- --print native-static-libs
```

Add anything it reports as a `.linkedLibrary(...)` entry in `Package.swift`.
