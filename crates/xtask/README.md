# xtask

The build system. An ordinary Rust binary that shells out to `cargo`, `swift`,
`xtool`, `codesign` and `container` — no Makefile, no shell scripts. Aliased
as `cargo xtask` in `.cargo/config.toml`.

| Command | What it does |
|---|---|
| `cargo xtask test` | `cargo nextest run --workspace` |
| `cargo xtask cov` | Coverage via `cargo llvm-cov`, fails under 100% lines, opens the HTML report (see [`TESTING.md`](../../TESTING.md)) |
| `cargo xtask ci` | `fmt --check` + `clippy -D warnings` + `test` + `cov` — what CI runs |
| `cargo xtask bindings` | Builds `remember-ffi`, runs `uniffi-bindgen`, and copies the generated Swift/header files into `swift/Sources/` |
| `cargo xtask build --target <triple>` | Builds the `remember-ffi` staticlib for one Apple target into `swift/Sources/RememberFFI/lib/` (one path, overwritten per target — no XCFramework, no `lipo`) |
| `cargo xtask mac` | `bindings` + `build` (macOS) + `swift build` + assembles `build/Remember.app` + ad-hoc `codesign` |
| `cargo xtask run` | `mac`, then `open build/Remember.app` |
| `cargo xtask sim` | Builds for the iOS Simulator, then `xtool dev --simulator` |
| `cargo xtask device` | Builds for an iOS device, then `xtool dev` (installs on a connected/paired device) |
| `cargo xtask package --version <x>` | `mac`, then zips the app into `build/Remember-<x>-macos-arm64.zip` via `ditto` and prints the path and sha256 (see [`RELEASING.md`](../../RELEASING.md)) |
| `cargo xtask pgtest` | Runs `crates/sql-tests` against a throwaway Postgres container (see [`crates/sql-tests`](../sql-tests/README.md)) |
| `cargo xtask web` | Builds the wasm demo into `site/pkg/` (see [`crates/web`](../web/README.md)) |

`mac`, `run`, `sim`, `device`, `package` and `pgtest` require macOS and
refuse to run anywhere else. The rest are plain Rust and work on any
platform.
