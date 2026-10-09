# remember-ffi

A thin [UniFFI](https://mozilla.github.io/uniffi-rs/) wrapper around
`remember_core::App`, exporting `Command`, `Snapshot`, `TaskRow`, `View`, and
`App` to Swift. Anything resembling a decision belongs in `remember-core`.

| File | What it does |
|---|---|
| `src/lib.rs` | The exported types. Every method body is a one-liner delegating to core — excluded from coverage, nothing to test |
| `src/convert.rs` | Every conversion between core types and their FFI mirrors. Held to 100% coverage |
| `src/bin/uniffi-bindgen.rs` | The bindgen binary `cargo xtask bindings` runs (needs the `cli` feature) |

The generated output lands in `swift/Sources/RememberKit/remember_ffi.swift`
and `swift/Sources/RememberFFI/` — never edit those by hand; rerun
`cargo xtask bindings` instead.
