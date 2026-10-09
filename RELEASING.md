# Releasing

Remember ships as a Homebrew cask ([`Casks/remember.rb`](Casks/remember.rb))
for Apple Silicon Macs. There's no App Store or TestFlight distribution.

## Cutting a release

Create and publish a GitHub Release (new or existing tag, draft or straight to
published — the workflow only cares about the `published` moment).
[`.github/workflows/release.yml`](.github/workflows/release.yml) then, on a
macOS runner:

1. runs `cargo xtask package --version <x>`, which builds the app and zips it
   into `build/Remember-<x>-macos-arm64.zip`,
2. uploads the zip as a release asset,
3. commits the updated version and sha256 back into `Casks/remember.rb`.

It's triggered by the `release` event specifically, not a tag push —
creating a release through the UI or `gh release create` doesn't reliably
fire a tag-push event, so the workflow listens for the thing you actually do.

Installed Macs pick it up with `brew update && brew upgrade --cask remember`.

## Signing

The app is ad-hoc signed (`codesign --sign -`) rather than through a paid
Developer ID. The cask strips the quarantine flag on install (`postflight` in
`Casks/remember.rb`) to avoid a Gatekeeper "unidentified developer" prompt.
That's an acceptable tradeoff for installing on Macs you own — it would not
be for distributing to other people.
