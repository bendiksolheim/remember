# Todo — implementation plan

A local-first todo app for macOS and iOS. Rust owns all logic and state; Swift
draws pixels. No Xcode project file, no second build system — `cargo xtask`
drives everything.

**This document is the entire specification.** An implementer with no prior
context should be able to work from it top to bottom.

---

## How to use this document

Work through the phases in order. Each ends with a **STOP** block: commands the
developer runs and things they verify by hand. Do not start the next phase
until the stop conditions pass.

Four standing instructions for anyone (human or agent) implementing this:

0. **Know which machine you are on.** Rust work happens in a Linux container
   with no Swift toolchain and no Apple SDK; anything that builds, signs, or
   runs the app happens on the macOS host. See §0.1. If you are the agent, you
   stop at each STOP and hand off.
1. **Verify library APIs before writing against them.** Version numbers and
   signatures here may be stale. Use `cargo add <crate>` to pull current
   versions, then check docs.rs for what actually resolved. Loro's API in
   particular has moved between releases. If a signature here disagrees with
   docs.rs, docs.rs wins.
2. **Tests are written with the code, not after it.** Every phase lists its
   required tests. `todo-core` is held at 100% line coverage from its first
   commit — see Appendix A for the policy and the reasoning.
3. **Do not invent scope.** If it isn't in this plan, it isn't in v1. Out of
   scope: sync, sharing, encryption, recurring tasks, tags, projects,
   notifications, widgets.

Placeholder to replace throughout: bundle ID `no.bendik.todo`.

### Phase overview

| # | Phase | Gate |
|---|---|---|
| 0 | Prerequisites | toolchain verified |
| 1 | Workspace skeleton | builds |
| 2 | Walking skeleton — macOS | **go/no-go 1** |
| 3 | Walking skeleton — iOS | **go/no-go 2** |
| 4 | Test infrastructure | coverage gate live |
| 5 | Core domain model | 100% coverage, properties hold |
| 6 | Persistence | survives restart |
| 7 | CLI + dogfooding | **one week of real use** |
| 8 | Full FFI surface | bindings correct |
| 9 | macOS UI | usable |
| 10 | iOS UI | usable on device |
| 11 | Sync seam | defined, unimplemented |

Phases 2 and 3 exist to fail fast. They build the real project structure with a
trivial payload, so every technically risky thing — UniFFI, SwiftPM linking,
xtool, code signing — is proven on day one, before there is any domain logic to
feel attached to. Nothing built in them is thrown away.

---

## Phase 0 — Prerequisites

### Required

| Thing | Why | Check |
|---|---|---|
| macOS on Apple Silicon | Build host | `uname -m` → `arm64` |
| Xcode.app (full install) | iOS SDKs only ship inside it | `xcodebuild -version` |
| Xcode CLT selected | Swift toolchain, macOS SDK | `xcode-select -p` |
| Rust via rustup | Everything | `rustc --version` |
| Swift 6.0+ | SwiftUI, Observation | `swift --version` |
| Apple Developer Program | Phase 3 device install | — |

You will never open Xcode.app. It is installed for its SDKs and toolchain only.

The $99/yr Developer Program is needed earlier here than you might expect,
because Phase 3 puts the app on a physical phone. Free personal provisioning
works but expires every 7 days.

### 0.1 Two environments

Development is split across a Linux container and the macOS host. The division
is not a preference — it is forced by what exists on each platform:

| | Container (Linux) | Host (macOS) |
|---|---|---|
| Rust: write, test, cover | ✅ | ✅ |
| `cargo xtask test / cov / ci` | ✅ | ✅ |
| `cargo xtask bindings` | ✅ | ✅ |
| Write Swift source files | ✅ | ✅ |
| Build Swift | ❌ | ✅ |
| `cargo xtask mac / run / sim / device` | ❌ | ✅ |

**Do not install a Swift toolchain in the container.** SwiftUI does not exist
on Linux — only Foundation and the core libraries are ported. Every Swift
target in this plan either imports SwiftUI (`TodoUI`, `TodoApp`, `TodoMac`) or
links a Darwin staticlib (`TodoKit`), so `swift build` on Linux fails on all of
them. A Swift toolchain there type-checks nothing.

The same holds one layer down: `rustup target add aarch64-apple-darwin` in the
container installs the standard library, but linking needs Apple's linker and
SDK. Cross-compiling the app from Linux is osxcross territory and not worth it
with a Mac available.

`cargo xtask bindings` is the one Apple-adjacent command that does work in the
container. UniFFI generates Swift source from interface metadata in the
compiled library, and a Linux `.so` serves that as well as a `.dylib` — so
xtask must pick the extension by platform rather than hardcoding `.dylib`.

This split lands exactly on the STOP points. An agent works up to a stop; the
developer runs the verification on the host.

### 0.2 Host setup (macOS)

```bash
rustup target add aarch64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim
rustup component add llvm-tools-preview

cargo install cargo-llvm-cov     # coverage
cargo install cargo-nextest      # faster test runner, better output

sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
```

### 0.3 Container setup (Linux)

Rust only. Added to whatever base image the agent runs in:

```dockerfile
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --profile minimal \
 && rustup component add clippy rustfmt llvm-tools-preview

# Prebuilt binaries — `cargo install` from source adds ~10 min to the image build
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
      https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh \
      | bash \
 && cargo binstall -y cargo-nextest cargo-llvm-cov cargo-mutants

# Keep container artifacts out of the bind-mounted workspace
ENV CARGO_TARGET_DIR="/home/<user>/.cargo-target"
```

Notes:

- `llvm-tools-preview` is easy to miss. `--profile minimal` omits it and
  `cargo-llvm-cov` fails without it.
- A C compiler is required for rusqlite's bundled SQLite (`build-essential` on
  Debian-family images). Loro is pure Rust and needs nothing extra.
- **`CARGO_TARGET_DIR` matters more than it looks.** Container and host both
  build for their *host* triple, so both would write to `target/debug/` in the
  shared workspace and invalidate each other's cache on every switch. Pointing
  the container's output outside the mount fixes the thrashing and gets faster
  I/O than a bind mount as a bonus.

> ### 🛑 STOP 0
>
> On the host:
>
> ```bash
> uname -m                       # arm64
> xcode-select -p                # /Applications/Xcode.app/Contents/Developer
> xcodebuild -version            # Xcode 16 or newer
> swift --version                # Swift 6.0 or newer
> rustup target list --installed # the three apple targets
> cargo llvm-cov --version
> cargo nextest --version
> xcrun --sdk iphonesimulator --show-sdk-path   # must print a path
> ```
>
> The last one is the real test — it proves the iOS SDK is present and
> reachable from the command line.
>
> In the container:
>
> ```bash
> cargo --version && cargo clippy --version
> cargo llvm-cov --version && cargo nextest --version
> echo $CARGO_TARGET_DIR         # must be set, and outside /workspace
> ```

---

## Phase 1 — Workspace skeleton

Work in the empty project folder alongside this document.

```bash
git init
cargo new --lib crates/core   --name todo-core
cargo new --lib crates/ffi    --name todo-ffi
cargo new --bin crates/cli    --name todo-cli
cargo new --bin crates/xtask  --name xtask
mkdir -p .cargo apple build \
  swift/Sources/{TodoFFI/lib,TodoKit,TodoUI,TodoApp,TodoMac}
```

### Final structure

```
todo/
├── PLAN.md                       # this document
├── CLAUDE.md                     # agent boundaries — see below
├── Cargo.toml                    # workspace root
├── .cargo/config.toml            # the `cargo xtask` alias
├── .gitignore
├── crates/
│   ├── core/                     # domain logic. No FFI, no Apple. 100% covered.
│   │   ├── src/{lib,doc,store,command,snapshot,clock,sync}.rs
│   │   └── tests/{integration,snapshots,properties,persistence}.rs
│   ├── ffi/                      # UniFFI wrappers. convert.rs covered; exports not.
│   │   └── src/{lib.rs,convert.rs,bin/uniffi-bindgen.rs}
│   ├── cli/                      # debug REPL. parser covered, I/O loop excluded.
│   └── xtask/                    # the build system. Not covered.
├── swift/
│   ├── Package.swift
│   ├── xtool.yml
│   └── Sources/
│       ├── TodoFFI/              # module.modulemap + header + staticlib (generated)
│       ├── TodoKit/              # generated bindings + Swift-side model
│       ├── TodoUI/               # SwiftUI views, shared macOS/iOS
│       ├── TodoApp/              # @main for iOS (library — xtool requires this)
│       └── TodoMac/              # @main for macOS (executable)
├── apple/
│   ├── Info-macOS.plist
│   ├── Info-iOS.plist
│   └── AppIcon.png
└── build/                        # assembled .app bundles. gitignored.
```

There is **no `.xcodeproj` anywhere**, and none will be created.

### Root `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = ["crates/core", "crates/ffi", "crates/cli", "crates/xtask"]

[workspace.package]
edition = "2021"
version = "0.1.0"

[profile.release]
lto = true
opt-level = "z"
```

### `.cargo/config.toml`

```toml
[alias]
xtask = "run --quiet --package xtask --"
```

### `.gitignore`

```
/target
/build
/swift/.build
/swift/Sources/TodoFFI/lib/
/swift/Sources/TodoFFI/*.h
/swift/Sources/TodoFFI/module.modulemap
/swift/Sources/TodoKit/todo_ffi.swift
/lcov.info
.DS_Store
```

Everything generated is gitignored. The generated Swift bindings are build
output, not source — never edit them by hand.

### `CLAUDE.md`

The plan assumes one machine; the agent is on the other one. Make the boundary
explicit at the repo root:

```markdown
# Working agreement

Read PLAN.md first. It is the specification. Follow it phase by phase.

## Environment

You run in a Linux container with Rust only. There is no Swift toolchain and no
Apple SDK. Do not try to install either — see PLAN.md §0.1 for why.

**You may:** write and test Rust; run `cargo xtask test`, `cov`, `ci`,
`bindings`; write Swift source files.

**You may not:** build or run the app. `cargo xtask mac | run | sim | device`
require macOS and will refuse to run here.

## Handoff

When a phase reaches a 🛑 STOP, stop. Summarise what you changed, state which
commands the developer needs to run on the host, and wait. Do not begin the
next phase.

## Non-negotiables

- `todo-core` stays at 100% line coverage. Tests ship with the code.
- No business logic in Swift. If a view needs to compute something, that
  computation belongs in `todo-core`.
- Loro is mentioned only in `crates/core/src/doc.rs`.
- No `.xcodeproj`. No `binaryTarget`. No XCFramework.
- Never edit generated files: `swift/Sources/TodoKit/todo_ffi.swift`,
  `swift/Sources/TodoFFI/*`.
```

### Crate lints

Top of `crates/core/src/lib.rs`:

```rust
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
```

Core must never panic. Every failure is a `CoreError`. This matters more than
usual here: a panic crossing the FFI boundary aborts the app process rather
than raising a Swift error.

> ### 🛑 STOP 1
>
> ```bash
> cargo build
> cargo metadata --no-deps --format-version 1 | grep -c '"name"'
> ```
>
> Verify: workspace compiles, four members listed.

---

## Phase 2 — Walking skeleton, macOS

Build the entire vertical slice — Rust through UniFFI through SwiftPM to a
running window — with a trivial payload. This proves the toolchain before any
domain logic exists.

### 2.1 `crates/ffi`

```bash
cd crates/ffi && cargo add uniffi --features cli && cd ../..
```

`crates/ffi/Cargo.toml`:

```toml
[lib]
name = "todo_ffi"
crate-type = ["lib", "staticlib", "cdylib"]

[[bin]]
name = "uniffi-bindgen"
path = "src/bin/uniffi-bindgen.rs"
```

Both crate types are required: `cdylib` for bindings generation (library mode
introspects a dylib), `staticlib` for linking into the app.

`crates/ffi/src/lib.rs`:

```rust
uniffi::setup_scaffolding!();

/// Diagnostic probe. Kept permanently — the fastest way to confirm which
/// architecture slice is actually linked into a running app.
#[uniffi::export]
pub fn build_info() -> String {
    format!(
        "todo {} / {} / {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        std::env::consts::OS
    )
}
```

`crates/ffi/src/bin/uniffi-bindgen.rs`:

```rust
fn main() { uniffi::uniffi_bindgen_main() }
```

### 2.2 xtask, minimal version

`crates/xtask` is an ordinary Rust binary shelling out via
`std::process::Command`. No Makefile, no shell scripts, no second language.

| Command | Action |
|---|---|
| `cargo xtask bindings` | build cdylib for host → run uniffi-bindgen → place files |
| `cargo xtask build --target <triple>` | build staticlib, copy to `swift/Sources/TodoFFI/lib/libtodo_ffi.a` |
| `cargo xtask mac` | bindings + build + `swift build` + assemble `.app` + sign |
| `cargo xtask run` | `mac`, then `open build/Todo.app` |

File placement performed by `bindings`:

| Generated file | Destination |
|---|---|
| `todo_ffi.swift` | `swift/Sources/TodoKit/todo_ffi.swift` |
| `todo_ffiFFI.h` | `swift/Sources/TodoFFI/todo_ffiFFI.h` |
| `todo_ffiFFI.modulemap` | `swift/Sources/TodoFFI/module.modulemap` ← **renamed** |

The rename is mandatory; SwiftPM only recognises `module.modulemap`.

Underlying commands, for reference:

```bash
cargo build -p todo-ffi --release
cargo run -p todo-ffi --bin uniffi-bindgen -- generate \
  --library target/release/libtodo_ffi.dylib \
  --language swift --out-dir <tmp> --no-format

cargo build -p todo-ffi --release --target aarch64-apple-darwin
cd swift && swift build -c release
```

There is exactly **one** staticlib path, overwritten with whichever target is
being built. No XCFramework, no `lipo`, no multi-slice bundle. An XCFramework
exists to ship multiple platform slices for *distribution*; you build one
target at a time for yourself, so it buys nothing and is the most fragile part
of the conventional setup.

`swift build` must run with `swift/` as the working directory — the `-L` path
in `Package.swift` is relative to the package root.

### Platform guard

`mac`, `run`, `sim`, and `device` require macOS. Guard them so the agent gets a
clear message instead of a confusing `xcodebuild: not found`:

```rust
fn require_macos(cmd: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!(
            "`cargo xtask {cmd}` requires macOS. \
             You are in the Linux container — hand off to the developer. \
             See PLAN.md §0.1."
        );
    }
    Ok(())
}
```

`bindings`, `test`, `cov`, and `ci` run on both platforms and are not guarded.
`bindings` must select `.dylib` or `.so` by `cfg!(target_os)` rather than
assuming Darwin.

Bundle assembly:

```
build/Todo.app/Contents/
├── Info.plist          # copied from apple/Info-macOS.plist
├── MacOS/Todo          # copied from swift/.build/release/TodoMac
└── Resources/
```

then `codesign --force --sign - build/Todo.app`. Ad-hoc signing is enough
locally; real entitlements (needed if sync later goes the iCloud route) require
a Developer certificate.

### 2.3 `swift/Package.swift`

```swift
// swift-tools-version: 6.0
import PackageDescription

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
                .unsafeFlags(["-LSources/TodoFFI/lib"]),
                .linkedLibrary("todo_ffi"),
            ]
        ),
        .target(name: "TodoUI", dependencies: ["TodoKit"]),
        .target(name: "TodoApp", dependencies: ["TodoUI"]),
        .executableTarget(name: "TodoMac", dependencies: ["TodoUI"]),
    ]
)
```

`.unsafeFlags` is prohibited in packages consumed as versioned dependencies but
allowed in a local root package, which this is. No `binaryTarget` appears
anywhere — that is deliberate, and the reason Phase 3 is expected to work.

`TodoApp` is a **library**, not an executable, because xtool bundles a library
product into an iOS app. `TodoMac` is a separate executable target with its own
`@main`. The ~10 lines of duplication is intentional.

### 2.4 `apple/Info-macOS.plist`

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>              <string>Todo</string>
  <key>CFBundleExecutable</key>        <string>Todo</string>
  <key>CFBundleIdentifier</key>        <string>no.bendik.todo</string>
  <key>CFBundlePackageType</key>       <string>APPL</string>
  <key>CFBundleVersion</key>           <string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key>    <string>14.0</string>
  <key>NSHighResolutionCapable</key>   <true/>
</dict>
</plist>
```

No asset catalog — `actool` lives inside Xcode.app rather than the command line
tools. Use a plain `.icns` in `Resources/` when you want an icon.

### 2.5 Swift stubs

`swift/Sources/TodoUI/TaskListView.swift`:

```swift
import SwiftUI
import TodoKit

public struct TaskListView: View {
    public init() {}
    public var body: some View {
        Text(buildInfo()).padding(40).monospaced()
    }
}
```

`swift/Sources/TodoMac/TodoMacApp.swift` — the filename matters. It must
**not** be `main.swift`; SwiftPM treats that name specially and it collides
with `@main`.

```swift
import SwiftUI
import TodoUI

@main
struct TodoMacApp: App {
    var body: some Scene {
        WindowGroup { TaskListView() }
            .defaultSize(width: 420, height: 640)
    }
}
```

### If linking fails

Undefined symbols mean the Rust staticlib needs system libraries SwiftPM isn't
passing. Ask Rust exactly which:

```bash
cargo rustc -p todo-ffi --release -- --print native-static-libs
```

Add each reported library as a `.linkedLibrary("…")` entry. Do not guess.

> ### 🛑 STOP 2 — go/no-go 1
>
> ```bash
> cargo xtask run
> ```
>
> Verify: a window opens showing `todo 0.1.0 / aarch64 / macos`, produced by
> Rust.
>
> If this was painful, stop and reconsider the whole approach before investing
> further. If it worked, the macOS half of the toolchain is done and will not
> need revisiting.

---

## Phase 3 — Walking skeleton, iOS

Same trivial payload, now on the phone. This is the riskiest part of the
project, which is why it happens now rather than at the end.

### 3.1 Install xtool

xtool is a cross-platform Xcode replacement that builds, signs, and deploys iOS
apps from a SwiftPM package. On macOS it uses the SDK from your Xcode install —
it removes the project file and `xcodebuild`, not Xcode itself.

```bash
brew install xtool          # check xtool-org/xtool for the current method
xtool --help
xtool setup                 # SDK + Apple Developer authentication
```

### 3.2 `swift/xtool.yml`

```yaml
version: 1
bundleID: no.bendik.todo
product: TodoApp
infoPath: ../apple/Info-iOS.plist
iconPath: ../apple/AppIcon.png      # 1024x1024 PNG
```

### 3.3 `swift/Sources/TodoApp/TodoIOSApp.swift`

```swift
import SwiftUI
import TodoUI

@main
public struct TodoIOSApp: App {
    public init() {}
    public var body: some Scene {
        WindowGroup { TaskListView() }
    }
}
```

### 3.4 xtask additions

- `cargo xtask sim` → build `aarch64-apple-ios-sim` staticlib → copy → `xtool dev --simulator`
- `cargo xtask device` → build `aarch64-apple-ios` staticlib → copy → `xtool dev`

Always get the simulator working first. The simulator does not enforce code
signing, which isolates build problems from signing problems.

### 3.5 If it doesn't work

xtool's historical weak spot is dependencies that aren't pure Swift source, and
SwiftPM has a long-standing issue where packages with a `.binaryTarget` fail to
link against the iOS SDK from the command line. The `systemLibrary` +
`.unsafeFlags` design avoids `binaryTarget` entirely, so this should hold.

**Fallback if it doesn't:** install XcodeGen, write `apple/project.yml`
describing a minimal iOS app target depending on the local SwiftPM package, and
have xtask run `xcodegen generate` then `xcodebuild`. Because every line of
real Swift lives in the package, the app shell is about twenty lines and
nothing is rewritten. The generated `.xcodeproj` stays gitignored.

Timebox this to an afternoon. Decide by trying, not by reading.

> ### 🛑 STOP 3 — go/no-go 2
>
> ```bash
> cargo xtask sim
> cargo xtask device
> ```
>
> Verify: both show `todo 0.1.0 / aarch64 / ios`. The string differing between
> platforms confirms the correct architecture slice is linked in each.
>
> **From here the toolchain is settled.** Everything after this is application
> code, and no remaining phase can fail for build-system reasons.

---

## Phase 4 — Test infrastructure

Set this up before writing domain logic, so coverage is never something you go
back and retrofit.

```bash
cd crates/core && cargo add --dev proptest insta tempfile && cd ../..
```

| Tool | Role |
|---|---|
| `cargo-nextest` | test runner — parallel, readable failures |
| `cargo-llvm-cov` | source-based coverage, accurate on Apple Silicon |
| `proptest` | property tests — the CRDT invariants |
| `insta` | snapshot tests — the `Snapshot` projection |
| `tempfile` | throwaway SQLite databases |

### xtask additions

```
cargo xtask test     # cargo nextest run --workspace
cargo xtask cov      # coverage report, opens HTML
cargo xtask ci       # fmt --check + clippy -D warnings + test + cov gate
```

`cargo xtask cov` runs:

```bash
cargo llvm-cov nextest \
  --package todo-core --package todo-ffi --package todo-cli \
  --ignore-filename-regex '(crates/ffi/src/lib\.rs|crates/cli/src/main\.rs)' \
  --fail-under-lines 100 \
  --html --open
```

See **Appendix A** for what those exclusions are and why blanket 100% across
the workspace is the wrong target.

### CI

`.github/workflows/ci.yml` — `cargo xtask ci` on push, running on
`macos-latest`. Rust only; no Swift build in CI, since the Swift layer has no
logic worth guarding and the runner minutes aren't worth it.

> ### 🛑 STOP 4
>
> ```bash
> cargo xtask ci
> ```
>
> Verify: passes trivially (nothing to cover yet). Then deliberately add an
> uncovered function to `todo-core` and confirm `cargo xtask cov` **fails**.
> A coverage gate you have never seen fail is not a gate.

---

## Phase 5 — Core domain model

The heart of the project. No Apple dependency, no FFI — developed entirely
through `cargo xtask test`.

```bash
cd crates/core
cargo add loro uuid thiserror
cargo add serde --features derive
```

### 5.1 Determinism first

Random UUIDs and wall-clock timestamps make snapshot and property tests flaky.
Inject both.

`src/clock.rs`:

```rust
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;   // unix seconds
}

pub trait IdSource: Send + Sync {
    fn new_id(&self) -> String;
}

pub struct SystemClock;
pub struct UuidSource;

#[cfg(any(test, feature = "testing"))]
pub struct FixedClock(pub i64);

#[cfg(any(test, feature = "testing"))]
pub struct SeqIdSource(std::sync::atomic::AtomicU64);
```

**Rule: `SystemTime::now()` and `Uuid::new_v4()` appear nowhere in `core`
except inside `SystemClock` and `UuidSource`.** Everything else takes
`Arc<dyn Clock>` / `Arc<dyn IdSource>`.

This single decision is what makes the coverage and property targets in this
plan achievable rather than aspirational. Expose the test doubles behind a
`testing` feature so integration tests in `tests/` can reach them.

### 5.2 `src/command.rs`

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Add { title: String, after: Option<String> },
    SetTitle { id: String, title: String },
    SetNotes { id: String, notes: String },
    SetDone { id: String, done: bool },
    SetDue { id: String, due: Option<i64> },
    Move { id: String, after: Option<String> },
    Delete { id: String },
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewFilter { #[default] All, Active, Completed }
```

`after: None` means "insert at the top".

### 5.3 `src/snapshot.rs`

What crosses the FFI boundary. Plain data: already filtered, already sorted,
already formatted. Swift does no computation.

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub done: bool,
    pub due: Option<i64>,
    pub due_label: Option<String>,   // "Today", "Tomorrow", "3 Mar"
    pub overdue: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Snapshot {
    pub rows: Vec<TaskRow>,
    pub view: ViewFilter,
    pub active_count: u32,
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
}
```

`Serialize` exists for `insta`, not for persistence.

### 5.4 `src/doc.rs`

**The only module that may mention Loro.** Confining it here means an API
mismatch is a localized fix rather than a rewrite.

Document layout:

- `tasks` — a `LoroMap` keyed by task UUID, each value a nested `LoroMap` with
  `title`, `notes`, `done`, `due`, `created_at`
- `order` — a `LoroMovableList` of task UUID strings

The `MovableList` is why Loro was chosen. Reordering is the only genuinely
conflict-prone operation in a todo app, and a movable list resolves concurrent
moves correctly. Do not replace it with an integer sort field or hand-rolled
fractional indexing.

```rust
pub struct Doc { doc: LoroDoc, undo: UndoManager }

impl Doc {
    pub fn new(peer_id: u64) -> Result<Self, CoreError>;
    pub fn load(peer_id: u64, snapshot: &[u8]) -> Result<Self, CoreError>;
    pub fn export_snapshot(&self) -> Result<Vec<u8>, CoreError>;
    pub fn apply(&mut self, cmd: Command, clock: &dyn Clock, ids: &dyn IdSource)
        -> Result<(), CoreError>;
    pub fn read(&self, view: ViewFilter, clock: &dyn Clock) -> Snapshot;
}
```

Implementation rules:

- **One `commit()` per user action**, so undo maps to a user-visible step.
- **No derived state in the document** — never store a computed sort index, a
  cached `overdue` flag, or a formatted date. Derive in `read()`.
- **Deletes remove from both** `tasks` and `order`.
- `read()` walks `order`, looks each id up in `tasks`, skips ids missing from
  `tasks` (defensive — possible after a future merge), applies the filter, and
  formats `due_label`.

> ⚠️ Check Loro's current API on docs.rs first. `set_peer_id`,
> `get_movable_list`, `mov`, `export`, and the `UndoManager` constructor have
> all changed across releases.

### 5.5 Errors

```rust
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CoreError {
    #[error("storage error: {0}")]  Storage(String),
    #[error("document error: {0}")] Document(String),
    #[error("no such task: {0}")]   NotFound(String),
}
```

Keep this small and stable — it becomes the FFI error type in Phase 8, and
churn there means regenerating bindings.

### 5.6 Tests

Four files. All are required to pass the Phase 5 gate.

#### `tests/integration.rs` — behaviour

One test per bullet, against the public API with `FixedClock` + `SeqIdSource`:

- add three tasks → `order` reflects insertion order
- `Add { after: None }` inserts at top; `after: Some(id)` inserts after that id
- `Add { after: Some(unknown) }` → `NotFound`
- `SetTitle` / `SetNotes` / `SetDue` update only the named field
- `SetDue(None)` clears the date and `overdue` becomes false
- `Move` to first, to last, to middle, and to its own position (no-op)
- `Move` with an unknown id → `NotFound`
- `Delete` removes from both maps; snapshot no longer contains it
- `Delete` of an unknown id → `NotFound`
- `SetDone(true)` + `ViewFilter::Active` hides it; `active_count` drops
- `ViewFilter::Completed` shows only completed
- `Undo` after each mutating command restores the prior snapshot
- `Redo` after `Undo` restores the post-command snapshot
- `Undo` on empty history is a no-op; `can_undo` is false
- `revision` strictly increases across every mutation
- empty document → empty rows, `active_count == 0`

#### `tests/snapshots.rs` — `insta`

Build a fixed document (fixed clock, sequential ids, ~6 tasks mixing
done/due/overdue) and snapshot the `Snapshot` under each `ViewFilter`. Also
snapshot `due_label` across a table of offsets: overdue, today, tomorrow, this
week, next month, next year, and `None`.

Date formatting is exactly the kind of code that is tedious to assert by hand
and trivial to review as a snapshot. Review the `.snap` files in diffs — an
unreviewed snapshot test asserts nothing.

#### `tests/properties.rs` — `proptest`

Write a `Command` generator producing sequences that reference only ids
existing at that point. Then assert:

1. **Convergence.** Two `Doc`s with *different* peer ids, given independent
   command sequences, cross-imported in either order, produce identical
   snapshots. *The most important test in the project.*
2. **Import idempotence.** Importing the same update bytes twice changes
   nothing.
3. **Import order independence.** A set of updates imported in any permutation
   converges to the same state.
4. **Snapshot round-trip.** `export_snapshot` → `load` → identical snapshot,
   for any command sequence.
5. **Structural consistency.** After any sequence, the key set of `tasks`
   exactly equals the set of ids in `order` — no orphans, no duplicates.
6. **Count invariant.** `active_count` always equals the number of rows with
   `!done` under `ViewFilter::All`.
7. **Undo inverts.** For any single command, `apply(c); undo()` yields the
   snapshot from before `c`.
8. **No panics.** Any sequence, including invalid ids, returns `Ok` or `Err` —
   never unwinds.

Property 1 proves the sync foundation works years before sync ships. If it is
awkward to express, the document layout in 5.4 is wrong — fix it now.

#### `src/**` — unit tests

`#[cfg(test)]` modules for the pure helpers: `due_label` boundary arithmetic,
filter predicates, `FixedClock`/`SeqIdSource` themselves. These exist to keep
integration tests focused on behaviour rather than edge-case enumeration.

> ### 🛑 STOP 5
>
> ```bash
> cargo xtask ci
> ```
>
> Verify: `todo-core` at 100% line coverage, all eight properties passing.
> Then run `cargo xtask cov` and actually **read the HTML report** — look for
> lines that are covered but not meaningfully asserted. Appendix B is how to
> check that mechanically.

---

## Phase 6 — Persistence

```bash
cd crates/core && cargo add rusqlite --features bundled && cargo add dirs
```

`bundled` compiles SQLite in — no system dependency, no version skew between
macOS and iOS.

### Schema — `src/store.rs`

```sql
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS doc (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  snapshot   BLOB    NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value BLOB NOT NULL
);
```

That is the whole schema. No normalized task tables — the Loro document is the
source of truth and the entire list fits in memory for any realistic personal
todo list. SQLite provides crash safety and, later, sync cursors. If querying
ever becomes a bottleneck, a materialized index table is an *addition*, not a
redesign.

### Peer ID

Read `meta['peer_id']`; if absent, generate a random `u64`, store it, return
it. **Stable for the life of the install, unique per device.** Two devices
sharing a peer id corrupt history in ways that are very hard to debug. It costs
nothing to get right now.

### Write policy

Debounced: mutations mark dirty; a background `std::thread` writes a fresh
snapshot after 2 seconds idle, inside a transaction. Explicit flush on
`App::flush()` (called from Swift on backgrounding) and on drop.

### Location

- macOS: `~/Library/Application Support/no.bendik.todo/todo.sqlite3`
- iOS: the app container's Application Support directory

Swift computes the path and passes it in; `core` never guesses. Keep it out of
any iCloud-synced directory for now — that would constrain the sync design
before it is made.

### Tests — `tests/persistence.rs`

Use `tempfile::TempDir` throughout; never touch a real user path.

- fresh DB → schema created, `peer_id` generated and non-zero
- reopen → **same** `peer_id`
- two separate DB paths → **different** `peer_id`s
- add tasks, drop `App`, reopen same path → identical snapshot
- explicit `flush()` then reopen without dropping → data present
- debounce: mutate, wait past the window, read from a second connection → data
  present
- opening a path in a non-existent directory → `CoreError::Storage`, no panic
- opening a file containing garbage → `CoreError::Storage`, no panic
- corrupt the stored blob → `CoreError::Document`, no panic
- `PRAGMA journal_mode` returns `wal`

The three "no panic" cases matter disproportionately. `core` is compiled with
`deny(clippy::unwrap_used)` precisely so a corrupt database surfaces as a Swift
error instead of aborting the app process.

> ### 🛑 STOP 6
>
> ```bash
> cargo xtask ci
> ```
>
> Verify: still 100% on `todo-core` with persistence included. Coverage
> typically slips here — error branches in storage code are easy to write and
> easy to leave untested.

---

## Phase 7 — CLI and dogfooding

```bash
cd crates/cli
cargo add todo-core --path ../core
cargo add clap --features derive
```

A thin REPL over the same `App`: `add <title>`, `ls`, `done <n>`, `rm <n>`,
`mv <n> <m>`, `undo`, `redo`, `quit`. Index by list position, not UUID — you
are typing these.

**Structure it for testability.** Parsing is a pure function:

```rust
pub fn parse(line: &str, snapshot: &Snapshot) -> Result<Option<Command>, ParseError>
```

`Ok(None)` for blank lines and `quit`. The loop in `main.rs` does nothing but
read a line, call `parse`, call `dispatch`, and render.

### Tests

`parse` covered at 100%: every command, wrong arity, non-numeric index,
out-of-range index, blank input, unknown command, and index-to-UUID resolution
against a snapshot. `main.rs` is excluded — it contains no decisions.

> ### 🛑 STOP 7 — the long one
>
> ```bash
> cargo run -p todo-cli
> ```
>
> **Use this as your actual todo list for a week.**
>
> The most valuable checkpoint in the plan, and the most tempting to skip. The
> interesting risk in this project was never technical — it is whether the data
> model matches how you actually want to manage tasks. You will discover
> missing concepts (or that half of `TaskRow` is dead weight) far more cheaply
> here than after building two UIs on top of it.
>
> Revise Phase 5 based on what you learn, keeping coverage at 100%. Then
> continue.

---

## Phase 8 — Full FFI surface

Replace the Phase 2 `build_info` stub with the real surface. Wrappers only —
anything resembling a decision belongs in `core`.

### `crates/ffi/src/convert.rs`

All conversions between core types and UniFFI types live here as plain
functions. **Covered at 100%.**

### `crates/ffi/src/lib.rs`

```rust
uniffi::setup_scaffolding!();

#[derive(uniffi::Enum)]   pub enum Command { /* mirrors core::Command */ }
#[derive(uniffi::Enum)]   pub enum View { All, Active, Completed }
#[derive(uniffi::Record)] pub struct TaskRow { /* … */ }
#[derive(uniffi::Record)] pub struct Snapshot { /* … */ }

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AppError {
    #[error("{message}")] Storage  { message: String },
    #[error("{message}")] Document { message: String },
    #[error("{message}")] NotFound { message: String },
}

#[uniffi::export(with_foreign)]
pub trait SnapshotListener: Send + Sync {
    fn on_change(&self, snapshot: Snapshot);
}

#[derive(uniffi::Object)]
pub struct App { /* wraps todo_core::App */ }

#[uniffi::export]
impl App {
    #[uniffi::constructor]
    pub fn open(db_path: String) -> Result<Arc<Self>, AppError>;
    pub fn subscribe(&self, listener: Arc<dyn SnapshotListener>);
    pub fn dispatch(&self, command: Command) -> Result<(), AppError>;
    pub fn set_view(&self, view: View);
    pub fn current(&self) -> Snapshot;
    pub fn flush(&self) -> Result<(), AppError>;
}
```

Design notes:

- **Everything is synchronous.** SQLite writes are sub-millisecond and the doc
  is in memory. Bridging Tokio through UniFFI is real work with no payoff yet.
- `subscribe` fires `on_change` immediately with the current snapshot, so Swift
  has no separate initial-load path.
- Full snapshots per change are fine at this scale. Add a diff variant only if
  it measurably stutters.
- No `Arc`, no opaque handles, no Loro types cross the boundary beyond `App`.

Each method body is a one-liner delegating to `core` via `convert.rs`. That is
what makes excluding `lib.rs` from coverage defensible: with no branching there
is nothing a test could find that review would not.

### Tests

- every `convert.rs` mapping, both directions, all enum variants and `Option`
  cases — 100%
- `CoreError` → `AppError` maps each variant to the right case, preserving the
  message

> ### 🛑 STOP 8
>
> ```bash
> cargo xtask ci
> cargo xtask bindings && head -50 swift/Sources/TodoKit/todo_ffi.swift
> ```
>
> Verify: coverage gate green; generated Swift contains `class App` with the
> methods above. Skim it — a type that looks wrong here will look wrong in
> SwiftUI too.

---

## Phase 9 — macOS UI

### `swift/Sources/TodoKit/Model.swift`

Hand-written, next to the generated `todo_ffi.swift`.

```swift
import Foundation
import Observation

public func defaultDatabasePath() -> String {
    let dir = FileManager.default
        .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("no.bendik.todo", isDirectory: true)
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.appendingPathComponent("todo.sqlite3").path
}

@MainActor
@Observable
public final class TodoModel {
    public private(set) var snapshot: Snapshot?
    private let app: App
    private var bridge: ListenerBridge?

    public init(dbPath: String = defaultDatabasePath()) throws {
        self.app = try App.open(dbPath: dbPath)
        let bridge = ListenerBridge { [weak self] snap in
            Task { @MainActor in self?.snapshot = snap }
        }
        self.bridge = bridge
        app.subscribe(listener: bridge)
    }

    public func dispatch(_ command: Command) {
        do { try app.dispatch(command: command) }
        catch { print("dispatch failed: \(error)") }
    }

    public func setView(_ view: View) { app.setView(view: view) }
    public func flush() { try? app.flush() }
}

private final class ListenerBridge: SnapshotListener, @unchecked Sendable {
    private let handler: (Snapshot) -> Void
    init(_ handler: @escaping (Snapshot) -> Void) { self.handler = handler }
    func onChange(snapshot: Snapshot) { handler(snapshot) }
}
```

UniFFI invokes `onChange` from whichever thread committed, so the `@MainActor`
hop is required, not optional.

### Views — `swift/Sources/TodoUI/`

- `TaskListView.swift` — list, swipe-to-delete, drag-to-reorder, inline add
  field, filter picker
- `TaskRowView.swift` — checkbox, title, due label
- `TaskDetailView.swift` — title, notes, due date

Rules:

- Views read `model.snapshot` and call `model.dispatch(...)`. Nothing else.
- No sorting, filtering, date formatting, or overdue logic in Swift — it all
  arrived in the snapshot. Wanting to compute something in a view means that
  computation belongs in `core`.
- Drag-to-reorder dispatches `Command.move(id:after:)`. Do not maintain a local
  reordered array; dispatch and let the next snapshot redraw.
- Platform differences confined to navigation chrome.

### Testing policy for Swift

No automated tests. The Swift layer holds no logic worth guarding, and every
rule above exists to keep it that way. If a Swift bug ever turns out to be a
logic bug, that logic was in the wrong language — move it to `core` and cover
it there. Treat "this needs a Swift test" as a design smell, not a test gap.

Update `TodoMacApp.swift` from Phase 2 to construct `TodoModel` and inject it
via `.environment(model)`.

> ### 🛑 STOP 9
>
> ```bash
> cargo xtask run
> ```
>
> Verify by hand: add, complete, edit, reorder, delete, undo, redo, filter.
> Quit and relaunch — everything persisted. Then run the CLI and confirm it
> sees the same state. Same database, two front ends.

---

## Phase 10 — iOS UI

The toolchain was settled in Phase 3, so this is wiring only: update
`TodoIOSApp.swift` to construct `TodoModel` and inject it exactly as macOS
does. `NavigationStack` on iOS where macOS uses `NavigationSplitView`; same
child views.

Add `.onChange(of: scenePhase)` calling `model.flush()` on background. iOS
kills suspended apps without warning and the 2-second debounce is not enough on
its own.

> ### 🛑 STOP 10
>
> ```bash
> cargo xtask sim && cargo xtask device
> ```
>
> Verify: full UI works on the phone. Background the app immediately after an
> edit, force-quit it, reopen — the edit survived. The two devices have
> separate databases and will not agree with each other. That is correct for
> v1.

---

## Phase 11 — The sync seam

Do not implement sync. Define its shape, so adding it later touches one crate
and introduces no new concepts.

`crates/core/src/sync.rs`:

```rust
/// Opaque Loro update bytes. The transport never interprets them.
pub trait SyncTransport: Send + Sync {
    fn push(&self, updates: Vec<u8>) -> Result<(), SyncError>;
    fn pull(&self, have: Vec<u8>) -> Result<Vec<u8>, SyncError>;
}

pub struct NoopTransport;
```

`todo_core::App` takes a `Box<dyn SyncTransport>`, defaulting to
`NoopTransport`. Add `Doc::export_updates_since(&VersionVector)` and
`Doc::import_updates(&[u8])` now — the Phase 5 property tests already exercise
exactly this pathway.

### Tests

`NoopTransport` covered. Plus a `FakeTransport` (in-memory, shared `Vec<u8>`)
driving two `App` instances through a full push/pull cycle and asserting
convergence, including: a push with nothing new is a no-op, a pull with nothing
new returns empty, and a transport returning an error leaves local state
untouched.

That suite is written now and stays green forever, which is how you will know
the seam still works whenever you get round to a real transport.

Because CRDT merges are commutative and idempotent, the transport needs no
ordering guarantees, no locking, and no server-side conflict logic. Options, in
rough preference order:

1. **iCloud Drive as a file container** — each device appends to its own
   `<peer-id>.loro`, reads the others, merges. The filesystem is the protocol.
   An afternoon of work, no server, no cost. Needs real entitlements.
2. **CKSyncEngine** — Apple's send/fetch coordination over CloudKit, designed
   for bring-your-own local store. Faster propagation, free push wake-ups, but
   the sync layer lives in Swift, costing testability.
3. **Self-hosted endpoint** — small Axum service, append-only op log,
   `GET /ops?since` + `POST /ops`, reachable over Tailscale so you can skip
   auth entirely.

Keep the choice out of `core`.

---

## Appendix A — Coverage policy

**100% of `todo-core`. Not 100% of the workspace.** A blanket workspace target
is the wrong goal: it forces contorted tests around code with no decisions in
it, and dilutes the signal from the code that has.

| Crate / file | Target | Rationale |
|---|---|---|
| `crates/core/**` | **100% lines, ≥95% regions**, enforced | All logic lives here |
| `crates/ffi/src/convert.rs` | **100%**, enforced | Pure mapping, easy to get wrong |
| `crates/ffi/src/lib.rs` | excluded | `#[uniffi::export]` scaffolding is unreachable from Rust tests; bodies are one-line delegations |
| `crates/cli/src/parse.rs` | **100%**, enforced | Pure function |
| `crates/cli/src/main.rs` | excluded | I/O loop, no decisions |
| `crates/xtask/**` | excluded | Build tooling, not application code |
| Swift | none | No logic by construction (Phase 9) |

The two exclusions in `ffi` and `cli` are only defensible because those files
are kept branch-free by design. **If a conditional appears in either, extract
it into a covered module.** That rule is the entire basis for the exclusion.

Region coverage sits below line coverage because `?` and `#[derive]` generate
branches no realistic test exercises. Chasing the last few percent of regions
produces worse tests, not better code. 95% is the honest ceiling; below that,
something real went untested.

Run `cargo xtask cov` and read the HTML report — don't just check the exit
code. The gate catches lines you forgot; only reading catches lines that ran
but were never asserted against.

---

## Appendix B — Making coverage mean something

100% line coverage proves every line ran. It does not prove any line was
checked. The cheap way to find out which:

```bash
cargo install cargo-mutants
cargo mutants --package todo-core
```

It mutates the code — flips comparisons, swaps operators, replaces return
values — and reports mutants the tests failed to kill. Every surviving mutant
is a line that is covered but not asserted.

Not part of the CI gate; it is slow. Run it once at Stop 5, once at Stop 6, and
whenever `doc.rs` changes substantially. Expect survivors to cluster around
`due_label` boundaries and `Move` index arithmetic.

---

## Appendix C — Speeding up the loop

Most iteration is `cargo xtask test`, with no Swift involved at all. That is by
design and where the time should go.

When you do need the app, static linking forces a full Swift relink on every
Rust change. If that becomes annoying, switch dev builds to a `cdylib`: build
`libtodo_ffi.dylib`, copy it into `Todo.app/Contents/Frameworks/`, add
`-rpath @executable_path/../Frameworks` to the link, and a Rust-only edit
becomes a Rust rebuild plus a relaunch. Keep `staticlib` for release.

Don't do this on day one. Measure first.

---

## Appendix D — Troubleshooting

**Undefined symbols when linking Swift.** The Rust staticlib needs system libs
SwiftPM isn't passing. Run
`cargo rustc -p todo-ffi --release -- --print native-static-libs` and add each
as `.linkedLibrary(...)`. Never guess.

**`swift build` can't find the module.** The modulemap must be named exactly
`module.modulemap`, and the header path inside it must match the header's
filename in the same directory.

**`-L` path not found.** `.unsafeFlags` paths are relative to the package root.
`swift build` must run with `swift/` as the working directory.

**`@main` conflicts with `main.swift`.** Rename the file containing your `App`
struct to anything else.

**App launches with no dock icon or window focus.** You ran the bare executable
instead of the assembled `.app`. Use `open build/Todo.app`.

**Wrong architecture linked.** There is one library path, overwritten per
target. `build_info()` prints the arch of the running slice — check it before
debugging anything else.

**Bindings out of sync with Rust.** `cargo xtask bindings`. Never hand-edit
`todo_ffi.swift`; it is build output.

**Flaky snapshot or property tests.** Something is calling `SystemTime::now()`
or `Uuid::new_v4()` outside `SystemClock` / `UuidSource`. See §5.1.

**Coverage drops after adding an error branch.** Expected. Every `CoreError`
construction site needs a test that triggers it. If one genuinely cannot be
triggered, the error variant is dead — delete it.

---

## Appendix E — Definition of done for v1

- Add, edit, complete, reorder, delete, undo, redo, filter
- Data survives quit and relaunch on both platforms
- macOS and iOS builds each run from a single `cargo` command
- `cargo xtask ci` green: fmt, clippy, tests, 100% on `todo-core`
- All eight properties in §5.6 passing
- `cargo mutants` run at least once with survivors reviewed
- No `.xcodeproj` in the repository
- Zero business logic in Swift
- `SyncTransport` defined, unimplemented, and covered by a fake-transport
  convergence test
