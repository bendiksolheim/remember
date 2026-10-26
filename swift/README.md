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

It also holds `Simulator.entitlements` and its DER encoding,
`Simulator.entitlements.der`, which `Package.swift` links into the iOS app's
`__TEXT,__entitlements` and `__TEXT,__ents_der` sections. The Simulator reads
an app's entitlements from there rather than from its code signature, and
`xtool dev --simulator` doesn't sign. Without them the Keychain fails with
-34018, so signing in is impossible. A device build ignores them: the
provisioning profile xtool signs with grants the real ones. `FAKETEAMID` is the
same placeholder team Xcode uses. After editing the plist, regenerate the DER
on a Mac:

```bash
derq query --raw -f xml -i apple/Simulator.entitlements -o apple/Simulator.entitlements.der
```

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
