# site

The static product page, deployed to GitHub Pages by
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on pushes to
`main` that touch the site or the crates it builds from.

The demo runs the real `remember-core` as wasm (via
[`crates/web`](../crates/web/README.md)), seeded with sample tasks on every
visit. Nothing is saved and there's no sync.

- `index.html`, `app.js` — rendering and input only; every rule lives in the
  core. The UI copies the macOS capture panel
  (`swift/Sources/RememberMac/CaptureView.swift`), so keep the two in step.
- `pkg/` — wasm output of `cargo xtask web`, gitignored and never committed.

Run locally:

```bash
cargo xtask web
python3 -m http.server -d site 8000
```
