// Everything on the page that isn't the demo. Kept apart from app.js so the
// download link works even before (or without) the wasm engine loading.

// Release assets carry the version in their name
// (`Remember-<x>-macos-arm64.zip`, see RELEASING.md), so there's no stable URL
// for the latest one. Ask GitHub for it; until then, or if that fails, the
// button keeps its fallback link to the latest release's page.
const REPO = "bendiksolheim/remember";

try {
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
    headers: { Accept: "application/vnd.github+json" },
  });
  if (res.ok) {
    const release = await res.json();
    const zip = release.assets?.find((a) => /-macos-arm64\.zip$/.test(a.name));
    if (zip) {
      document.getElementById("download").href = zip.browser_download_url;
      const version = release.tag_name.replace(/^v/, "");
      document.getElementById("download-meta").textContent =
        `Version ${version} · macOS 14 or later · Apple Silicon`;
    }
  }
} catch (e) {
  console.error(e);
}
