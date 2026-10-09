# Remember

Reminder is a local-first todo app, with optional sync between your devices. Create lists, add
tasks and set due dates when you need them. All of it just one keystroke away, always.

Simplicity and availability are top priorities. We value well thought out features that fits
well with other functionality and can play a central role in the application. Remember should
never be more than one keystroke away, and be ready to accept new tasks immedeately.

The interface is clear and easy to understand. Everything can be navigated by keyboard.

## Installation

Manually by [downloading a release](https://github.com/bendiksolheim/remember/releases), or
with Homebrew:

```bash
brew tap bendiksolheim/tap https://github.com/bendiksolheim/remember
brew install --cask remember
```

Update with `brew update && brew upgrade --cask remember`. Why the app is
ad-hoc signed and how releases are cut: [`RELEASING.md`](RELEASING.md).

## Technical decisions

The code is split between Rust and other languages. Rust owns the core – the "engine". Functionality
related to tasks and lists should always be implemented as part of the engine, and exposed to the
host. Apps are always implemented in a hosts native language and gui framework, meaning Swift for
macOS and iOS. The engine is compiled for the host and exposed with UniFFi. This makes every
platform run the same code under the hood.

## Layout

| Folder | What it is |
|---|---|
| [`crates/core`](crates/core/README.md) | `remember-core` — all domain logic, the Loro CRDT document, SQLite persistence |
| [`crates/ffi`](crates/ffi/README.md) | `remember-ffi` — thin UniFFI wrapper exporting `remember_core::App` to Swift |
| [`crates/sync`](crates/sync/README.md) | `remember-sync` — opt-in device sync against Supabase |
| [`crates/web`](crates/web/README.md) | `remember-web` — wasm wrapper powering the product page demo |
| [`crates/xtask`](crates/xtask/README.md) | The build system: every `cargo xtask` command |
| [`crates/sql-tests`](crates/sql-tests/README.md) | Tests for the Supabase SQL functions against a real Postgres |
| [`swift`](swift/README.md) | The SwiftPM package: generated bindings, SwiftUI views, macOS/iOS entry points |
| [`site`](site/README.md) | Static product page with a live wasm demo, deployed to GitHub Pages |
| [`supabase`](supabase/README.md) | Sync backend schema and setup recipe |
| `apple/` | `Info.plist`s and the app icon (see [`swift/README.md`](swift/README.md#apple)) |
| `Casks/` | Homebrew cask (see [`RELEASING.md`](RELEASING.md)) |

Also: [`TESTING.md`](TESTING.md) for the test strategy and coverage policy,
[`RELEASING.md`](RELEASING.md) for cutting a release.

## Prerequisites

All app builds happen on macOS (Apple Silicon), because compiling the app
requires the Apple SDKs. The Rust-only commands (`test`, `cov`, `bindings`,
`web`) work anywhere.

| Tool | Why |
|---|---|
| Xcode.app (full install) | Provides the iOS/macOS SDKs and Swift toolchain — you never open it directly |
| Xcode Command Line Tools selected (`xcode-select -p`) | |
| Rust via [rustup](https://rustup.rs), with the `aarch64-apple-darwin`, `aarch64-apple-ios`, `aarch64-apple-ios-sim` targets added | |
| `cargo-nextest`, `cargo-llvm-cov` | Test runner + coverage, used by `cargo xtask test`/`cov`/`ci` |
| [`xtool`](https://github.com/xtool-org/xtool) | Builds/signs/installs the iOS app without Xcode's project system — run `xtool setup` once to authenticate |
| An Apple Developer account | Needed to install the iOS build on a physical device (free personal provisioning works, but expires every 7 days) |

## Quickstart

```bash
cargo xtask bindings   # first time, and whenever the FFI surface changes
cargo xtask run        # build and open the macOS app
cargo xtask sim        # iOS Simulator
cargo xtask device     # physical iOS device (needs `xtool setup` and a paired device)
cargo xtask test       # run the test suite
```

The full command reference is in [`crates/xtask/README.md`](crates/xtask/README.md).
