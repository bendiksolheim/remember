# site

The static product page, deployed to GitHub Pages by
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on pushes to
`main` that touch the site or the crates it builds from.

The demo runs the real `remember-core` as wasm (via
[`crates/web`](../crates/web/README.md)), seeded with sample tasks on every
visit. Nothing is saved and there's no sync.

- `index.html` — the page: hero, the demo panel, the values (the opening of
  the [root README](../README.md), keep the two telling the same story) and
  install instructions.
- `app.js` — the demo. Rendering and input only; every rule lives in the
  core. The UI copies the macOS capture panel
  (`swift/Sources/RememberMac/CaptureView.swift`), so keep the two in step.
- `page.js` — everything else that runs: points the download button at the
  latest release's zip via the GitHub API, falling back to the releases page.
- `icon.png` — a copy of `apple/AppIcon.png`.
- `pkg/` — wasm output of `cargo xtask web`, gitignored and never committed.

Run locally:

```bash
cargo xtask web
python3 -m http.server -d site 8000
```
