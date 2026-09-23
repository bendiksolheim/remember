# Todo

A local-first todo app for macOS and iOS. Rust owns all logic and state; Swift
only draws pixels. There is no Xcode project — `cargo xtask` drives the whole
build.

This file is the practical "how do I build and run this" reference. There is
no longer a standing design-spec document (`plans/PLAN.md` was retired once
the initial build was done) — architecture decisions since then are captured
in code comments at the point they matter, plus [`supabase/README.md`](supabase/README.md)
for the sync backend specifically.

## Architecture

- **`crates/core` (`todo-core`)** — all domain logic. Tasks are stored in a
  [Loro](https://loro.dev) CRDT document (`src/doc.rs` is the only file that
  mentions Loro), persisted to SQLite as an opaque snapshot blob
  (`src/store.rs`). `todo_core::App` ties the document, storage, and a
  debounced background writer together, and is the type every frontend
  (Swift, CLI) actually talks to.
- **`crates/ffi` (`todo-ffi`)** — a thin [UniFFI](https://mozilla.github.io/uniffi-rs/)
  wrapper around `todo_core::App`, exporting `Command`, `Snapshot`, `TaskRow`,
  `View`, and `App` to Swift. `src/convert.rs` holds every conversion between
  core types and their FFI mirrors and is the only file in this crate held to
  100% test coverage — `src/lib.rs` is pure delegation, nothing to test.
- **`crates/cli` (`todo-cli`)** — a debug REPL over the same `todo_core::App`
  used for early dogfooding. Not a shipping product surface.
- **`crates/xtask`** — the build system (see [Commands](#commands) below).
  An ordinary Rust binary that shells out to `cargo`, `swift`, `xtool`, and
  `codesign` — no Makefile, no shell scripts.
- **`swift/`** — a single SwiftPM package (`Todo`) with no `.xcodeproj`, no
  `binaryTarget`, no XCFramework:
  - `TodoFFI` — a `systemLibrary` target wrapping the generated C header +
    static lib.
  - `TodoKit` — generated bindings (`todo_ffi.swift`, **never hand-edited**)
    plus `Model.swift`, the hand-written `TodoModel` that bridges
    `todo_core::App` into an `@Observable` SwiftUI-friendly model.
  - `TodoUI` — SwiftUI views, shared between macOS and iOS. Views read
    `model.snapshot` and call `model.dispatch(...)`; no sorting, filtering,
    date formatting, or reordering logic lives here — all of that arrives
    pre-computed in the snapshot from `todo-core`.
  - `TodoMac` / `TodoApp` — the macOS and iOS `@main` entry points. The only
    place platform differences (navigation chrome, app lifecycle) are
    allowed to live. **Status:** `TodoMac` constructs `TodoModel` and injects
    it via `.environment(model)`; `TodoApp` (iOS) is still the placeholder
    from the early walking-skeleton phase and hasn't been updated to do the
    same yet — running `cargo xtask sim`/`device` right now will crash at
    launch (`TaskListView` expects `TodoModel` in the environment). This is
    the next thing to fix.
- **`apple/`** — `Info.plist` sources and the app icon. Bundle ID:
  `no.bendik.todo`.
- **`crates/sync` (`todo-sync`)** — device sync: pushes/pulls Loro
  update-blobs to/from a Supabase project over `reqwest::blocking`, plus
  auth (`AuthClient`) and orchestration (`SyncEngine`). Opt-in — the app
  works fully with no backend configured. See
  [`supabase/README.md`](supabase/README.md) for standing up the backend
  this crate talks to, and [`supabase/schema.sql`](supabase/schema.sql) for
  the actual table/RLS definitions.

### Why Rust *and* Swift

Rust owns every decision (what a "task" is, sorting, filtering, date
formatting, undo/redo, sync). Swift is presentation-only. If a SwiftUI view
ever needs to *compute* something rather than just display it, that logic
belongs in `todo-core`, not in the view — the one narrow exception being
something that's genuinely specific to a single platform's chrome (e.g.
`NavigationStack` vs. a split view).

## Prerequisites

All builds happen on macOS (Apple Silicon), because compiling the app for
real requires the Apple SDKs — there's no way around that.

| Tool | Why |
|---|---|
| Xcode.app (full install) | Provides the iOS/macOS SDKs and Swift toolchain — you never open it directly |
| Xcode Command Line Tools selected (`xcode-select -p`) | |
| Rust via [rustup](https://rustup.rs), with the `aarch64-apple-darwin`, `aarch64-apple-ios`, `aarch64-apple-ios-sim` targets added | |
| `cargo-nextest`, `cargo-llvm-cov` | Test runner + coverage, used by `cargo xtask test`/`cov`/`ci` |
| [`xtool`](https://github.com/xtool-org/xtool) | Builds/signs/installs the iOS app without Xcode's project system — run `xtool setup` once to authenticate |
| An Apple Developer account | Needed to install the iOS build on a physical device (free personal provisioning works, but expires every 7 days) |

## Commands

Everything goes through `cargo xtask`, aliased in `.cargo/config.toml`:

| Command | What it does |
|---|---|
| `cargo xtask test` | `cargo nextest run --workspace` |
| `cargo xtask cov` | Coverage report via `cargo llvm-cov`, opens the HTML report |
| `cargo xtask ci` | `fmt --check` + `clippy -D warnings` + `test` + `cov` gate — what CI runs |
| `cargo xtask bindings` | Builds `todo-ffi`, runs `uniffi-bindgen`, and copies the generated Swift/header files into `swift/Sources/` |
| `cargo xtask build --target <triple>` | Builds the `todo-ffi` staticlib for one Apple target and copies it into `swift/Sources/TodoFFI/lib/` (there's exactly one staticlib path, overwritten per target — no XCFramework, no `lipo`) |
| `cargo xtask mac` | `bindings` + `build` (macOS target) + `swift build` + assembles `build/Todo.app` + ad-hoc `codesign` |
| `cargo xtask run` | `mac`, then `open build/Todo.app` |
| `cargo xtask sim` | Builds for the iOS Simulator target, then `xtool dev --simulator` |
| `cargo xtask device` | Builds for the iOS device target, then `xtool dev` (installs on a connected/paired device) |
| `cargo xtask package --version <x>` | `mac`, then archives `build/Todo.app` into `build/Todo-<x>-macos-arm64.zip` via `ditto` and prints the zip path and its sha256 — what CI uses to cut a release |

`mac`, `run`, `sim`, and `device` all require macOS and refuse to run
anywhere else. `bindings`, `test`, `cov`, and `ci` are plain Rust and work on
any platform.

## Building and running

```bash
# First time only (or whenever the FFI surface changes):
cargo xtask bindings

# macOS:
cargo xtask run

# iOS Simulator (currently crashes at launch — see the TodoApp status note above):
cargo xtask sim

# Physical iOS device (needs `xtool setup` done once, and a paired device):
cargo xtask device
```

`cargo xtask mac`/`run` produce `build/Todo.app`, ad-hoc signed
(`codesign --sign -`) — enough to run locally. There's no App Store or
TestFlight distribution set up; ad-hoc signing is only good enough for
installs on Macs the developer owns (see [Installing via Homebrew](#installing-via-homebrew)
below), not public distribution.

## Installing via Homebrew

Other Macs (Apple Silicon only) can install the built app without any of the
above toolchain — just [Homebrew](https://brew.sh):

```bash
brew tap bendiksolheim/tap https://github.com/bendiksolheim/remember
brew install --cask todo
```

The app ships ad-hoc signed rather than through a paid Developer ID, so the
Cask strips the quarantine flag on install (`postflight` in
[`Casks/todo.rb`](Casks/todo.rb)) to avoid a Gatekeeper "unidentified
developer" prompt. That's an acceptable tradeoff for installing on Macs you
own — it would not be for distributing to other people.

To cut a new release: create and publish a GitHub Release (new or existing
tag, draft or straight to published — the workflow only cares about the
`published` moment). [`.github/workflows/release.yml`](.github/workflows/release.yml)
then builds and packages the app on a macOS runner, uploads the zip as a
release asset, and commits the updated version/sha256 back into
`Casks/todo.rb`. It's triggered by the `release` event specifically (not a
tag push) — creating a release through the UI or `gh release create` doesn't
reliably fire a tag-push event, so the workflow listens for the thing you
actually do.

Then, on each install-only Mac: `brew update && brew upgrade --cask todo`.

### If a Swift build fails to link

The Rust staticlib needs whatever system libraries `rustc` linked against.
Never guess — ask it:

```bash
cargo rustc -p todo-ffi --release -- --print native-static-libs
```

Add anything reported as a `.linkedLibrary(...)` entry in `swift/Package.swift`.

## Testing

- `cargo xtask test` / `cargo xtask ci` run everything: unit tests, the
  `tests/integration.rs` behavioral suite, `tests/properties.rs`
  (proptest — property 1, convergence of independently-edited CRDT replicas
  merged in either order, is the most important test in the project),
  `tests/snapshots.rs` (`insta`, reviewed by hand, not just accepted),
  and `tests/persistence.rs` (SQLite round-trips, using `tempfile` — never a
  real user path).
- `todo-core` is held to 100% line coverage in policy (see `CLAUDE.md` for
  the rule and the couple of narrow, deliberate file exclusions in
  `crates/xtask/src/main.rs`'s `cov()`). The gate in `xtask` actually checks
  `--fail-under-lines 99`,
  not 100 — a `cargo-llvm-cov` reporting quirk in one environment makes a
  couple of lines show as "missed" even though the HTML report, LCOV export,
  and manual inspection all confirm they run; see the comment on `cov()` in
  `crates/xtask/src/main.rs` before assuming a real gap opened up.
  `cargo xtask cov` opens an HTML report — reading it occasionally, not just
  checking the exit code, is how you catch a line that ran but was never
  actually asserted against.
- `cargo mutants -p todo-core --features todo-core/testing` runs mutation
  testing against the core crate. Not part of the CI gate (slow); worth
  rerunning whenever `doc.rs` or the date-formatting code changes
  substantially.
- Swift has no automated tests by design. If a Swift bug ever turns out to
  be a logic bug, that logic was in the wrong language; move it to
  `todo-core` and cover it there.

## Data & persistence

- macOS: `~/Library/Application Support/no.bendik.todo/todo.sqlite3`
- iOS: the app container's own Application Support directory
- CLI: platform data directory (via the `dirs` crate) — run `todo-cli
  <path>` to point it at a specific file instead (handy for testing)

The SQLite file holds exactly two things: the latest exported Loro snapshot
(`doc` table) and a random per-install peer id (`meta` table, generated once,
stable for the life of the install — never share a peer id across devices).
There are no normalized task tables; the CRDT document is the source of
truth, and the whole list comfortably fits in memory for any real personal
todo list.

## CLI

`cargo run -p todo-cli [db-path]` opens a REPL using the same `todo_core::App`
type the GUI uses, and can point at the same database file — though undo/redo
history is per-process (it's not persisted to disk), so it won't carry over
between the CLI and a running GUI instance even when both target the same
file:

```
add <title>       ls                done <n>
rm <n>            mv <n> <m>        undo / redo
quit
```

Indices are 1-based positions in the currently-displayed list, not UUIDs.
`mv <n> <m>` moves item `n` to right after item `m` (relational, matching
`Command::Move`'s own semantics — the same primitive drag-and-drop in the
GUI uses) — not "to position `m`".

## Out of scope (v1)

Sharing, end-to-end encryption, recurring tasks, tags, projects,
notifications, and widgets are all deliberately not implemented. Device sync
exists (`crates/sync`, see above) but is plaintext-at-rest and single-owner
only — no collaboration/shared lists.
