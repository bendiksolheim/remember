# remember-web

A wasm-bindgen wrapper around `remember_core::Session` — the engine behind the
demo on the product page ([`site/`](../../site/README.md)). Glue only:
commands arrive as plain JS objects shaped like `Command`
(`{ type: "Add", title, ... }`) and snapshots leave the same way. Persistence
is the host page's job.

## Building

```bash
cargo xtask web
python3 -m http.server -d site 8000
```

`cargo xtask web` builds for `wasm32-unknown-unknown` with `[profile.web]`
(release plus `panic = "abort"`, stripped) and runs `wasm-bindgen` into
`site/pkg/` (gitignored).

Needs:

- `rustup target add wasm32-unknown-unknown`
- `wasm-bindgen-cli` at the same version as `wasm-bindgen` in `Cargo.lock`
