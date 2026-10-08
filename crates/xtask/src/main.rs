use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next();
    let rest: Vec<String> = args.collect();

    match cmd.as_deref() {
        Some("bindings") => bindings(),
        Some("build") => build(&rest),
        Some("mac") => mac(),
        Some("run") => run(),
        Some("sim") => sim(),
        Some("device") => device(),
        Some("package") => package(&rest),
        Some("test") => test(),
        Some("cov") => cov(),
        Some("ci") => ci(),
        Some("pgtest") => pgtest(),
        Some("web") => web(),
        Some(other) => bail!("unknown xtask command: {other}"),
        None => bail!(
            "usage: cargo xtask <bindings|build|mac|run|sim|device|package|test|cov|ci|pgtest|web>"
        ),
    }
}

fn require_macos(cmd: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!(
            "`cargo xtask {cmd}` requires macOS. \
             You are in the Linux container — hand off to the developer."
        );
    }
    Ok(())
}

fn run_cmd(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn {cmd:?}"))?;
    if !status.success() {
        bail!("command failed ({status}): {cmd:?}");
    }
    Ok(())
}

struct Metadata {
    target_directory: PathBuf,
    workspace_root: PathBuf,
}

fn cargo_metadata() -> Result<Metadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .context("failed to spawn cargo metadata")?;
    if !output.status.success() {
        bail!("cargo metadata failed");
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let target_directory = json["target_directory"]
        .as_str()
        .ok_or_else(|| anyhow!("cargo metadata: missing target_directory"))?
        .into();
    let workspace_root = json["workspace_root"]
        .as_str()
        .ok_or_else(|| anyhow!("cargo metadata: missing workspace_root"))?
        .into();
    Ok(Metadata {
        target_directory,
        workspace_root,
    })
}

fn dylib_extension() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// bindings + build (staticlib) + `swift build` + assemble `.app` + sign.
fn bindings() -> Result<()> {
    let meta = cargo_metadata()?;

    run_cmd(Command::new("cargo").args(["build", "-p", "remember-ffi", "--release"]))?;

    let lib_path = meta
        .target_directory
        .join("release")
        .join(format!("libremember_ffi.{}", dylib_extension()));

    let out_dir = meta.target_directory.join("uniffi-bindgen-out");
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    run_cmd(
        Command::new("cargo").args([
            "run",
            "-p",
            "remember-ffi",
            "--features",
            "cli",
            "--bin",
            "uniffi-bindgen",
            "--",
            "generate",
            "--library",
            lib_path
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 path: {}", lib_path.display()))?,
            "--language",
            "swift",
            "--out-dir",
            out_dir
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 path: {}", out_dir.display()))?,
            "--no-format",
        ]),
    )?;

    let swift_dir = meta.workspace_root.join("swift");
    place_generated_file(
        &out_dir.join("remember_ffi.swift"),
        &swift_dir.join("Sources/RememberKit/remember_ffi.swift"),
    )?;
    place_generated_file(
        &out_dir.join("remember_ffiFFI.h"),
        &swift_dir.join("Sources/RememberFFI/remember_ffiFFI.h"),
    )?;
    place_generated_file(
        &out_dir.join("remember_ffiFFI.modulemap"),
        &swift_dir.join("Sources/RememberFFI/module.modulemap"),
    )?;

    Ok(())
}

fn place_generated_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::copy(from, to)
        .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
    Ok(())
}

/// Builds the browser demo engine into `site/pkg` (gitignored); serve
/// `site/` with any static file server to try it. The Pages workflow runs
/// this too. Needs the `wasm32-unknown-unknown` target and a `wasm-bindgen`
/// CLI matching the `wasm-bindgen` version in Cargo.lock.
fn web() -> Result<()> {
    let meta = cargo_metadata()?;

    run_cmd(Command::new("cargo").args([
        "build",
        "-p",
        "remember-web",
        "--profile",
        "web",
        "--target",
        "wasm32-unknown-unknown",
    ]))?;

    let wasm = meta
        .target_directory
        .join("wasm32-unknown-unknown/web/remember_web.wasm");
    let out_dir = meta.workspace_root.join("site/pkg");
    run_cmd(
        Command::new("wasm-bindgen")
            .args(["--target", "web", "--out-dir"])
            .arg(&out_dir)
            .arg(&wasm),
    )?;

    println!("Built {}. Serve site/, e.g.:", out_dir.display());
    println!("  python3 -m http.server -d site 8000");
    Ok(())
}

/// Build the remember-ffi staticlib for one Apple target triple and copy it into
/// the SwiftPM tree. There is exactly one staticlib path, overwritten with
/// whichever target was built last — no XCFramework, no `lipo`.
fn build(args: &[String]) -> Result<()> {
    let target = parse_target_flag(args)?;
    let meta = cargo_metadata()?;

    run_cmd(Command::new("cargo").args([
        "build",
        "-p",
        "remember-ffi",
        "--release",
        "--target",
        &target,
    ]))?;

    let lib_path = meta
        .target_directory
        .join(&target)
        .join("release")
        .join("libremember_ffi.a");
    let dest = meta
        .workspace_root
        .join("swift/Sources/RememberFFI/lib/libremember_ffi.a");
    place_generated_file(&lib_path, &dest)?;

    Ok(())
}

fn parse_target_flag(args: &[String]) -> Result<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--target=") {
            return Ok(value.to_string());
        }
        if arg == "--target" {
            return iter
                .next()
                .cloned()
                .ok_or_else(|| anyhow!("--target requires a value"));
        }
    }
    bail!("usage: cargo xtask build --target <triple>")
}

/// bindings + build + `swift build` + assemble `Remember.app` + ad-hoc sign.
fn mac() -> Result<()> {
    mac_with_version(None)
}

/// `mac`, with a release's `version` written into the bundle's Info.plist.
/// It has to happen here, before signing: editing the plist afterwards
/// would invalidate the signature. `None` keeps the checked-in plist's
/// "dev" values.
fn mac_with_version(version: Option<&str>) -> Result<()> {
    require_macos("mac")?;

    let meta = cargo_metadata()?;

    bindings()?;
    build(&["--target".to_string(), "aarch64-apple-darwin".to_string()])?;

    // Scoped to the RememberMac product: `swift build` otherwise builds every
    // target in the package, including RememberApp — the iOS entry point, whose
    // source doesn't exist until Phase 3 — and fails on its empty target.
    run_cmd(
        Command::new("swift")
            .args(["build", "-c", "release", "--product", "RememberMac"])
            .current_dir(meta.workspace_root.join("swift")),
    )?;

    assemble_app(&meta, version)?;

    run_cmd(
        Command::new("codesign").args([
            "--force",
            "--sign",
            "-",
            meta.workspace_root
                .join("build/Remember.app")
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 workspace root"))?,
        ]),
    )?;

    Ok(())
}

fn assemble_app(meta: &Metadata, version: Option<&str>) -> Result<()> {
    let app = meta.workspace_root.join("build/Remember.app/Contents");
    let macos_dir = app.join("MacOS");
    let resources_dir = app.join("Resources");
    fs::create_dir_all(&macos_dir).with_context(|| format!("creating {}", macos_dir.display()))?;
    fs::create_dir_all(&resources_dir)
        .with_context(|| format!("creating {}", resources_dir.display()))?;

    let plist = fs::read_to_string(meta.workspace_root.join("apple/Info-macOS.plist"))
        .context("reading Info-macOS.plist")?;
    let plist = match version {
        Some(version) => stamp_version(&plist, version)?,
        None => plist,
    };
    fs::write(app.join("Info.plist"), plist).context("writing Info.plist")?;

    fs::copy(
        meta.workspace_root.join("swift/.build/release/RememberMac"),
        macos_dir.join("Remember"),
    )
    .context("copying RememberMac executable")?;

    Ok(())
}

/// `mac`, then open the assembled bundle.
fn run() -> Result<()> {
    require_macos("run")?;
    let meta = cargo_metadata()?;
    mac()?;
    run_cmd(Command::new("open").arg(meta.workspace_root.join("build/Remember.app")))?;
    Ok(())
}

/// Build the iOS-simulator staticlib and launch via xtool. Assumes
/// `cargo xtask bindings` has already been run at least once — the generated
/// Swift API doesn't change per platform, only the staticlib triple does.
fn sim() -> Result<()> {
    require_macos("sim")?;
    let meta = cargo_metadata()?;
    build(&["--target".to_string(), "aarch64-apple-ios-sim".to_string()])?;
    run_cmd(
        Command::new("xtool")
            .args(["dev", "--simulator"])
            .current_dir(meta.workspace_root.join("swift")),
    )?;
    Ok(())
}

/// Build the iOS-device staticlib and launch via xtool.
fn device() -> Result<()> {
    require_macos("device")?;
    let meta = cargo_metadata()?;
    build(&["--target".to_string(), "aarch64-apple-ios".to_string()])?;
    run_cmd(
        Command::new("xtool")
            .args(["dev"])
            .current_dir(meta.workspace_root.join("swift")),
    )?;
    Ok(())
}

/// `mac`, then archive the signed `.app` into a release zip and print its
/// path and sha256 (one line each), for CI to read back when cutting a
/// GitHub Release / updating the Cask.
fn package(args: &[String]) -> Result<()> {
    require_macos("package")?;
    let version = parse_version_flag(args)?;
    // Before the slow build, not when the plist is written at the end of it.
    validate_version(&version)?;
    let meta = cargo_metadata()?;

    mac_with_version(Some(&version))?;

    let app_path = meta.workspace_root.join("build/Remember.app");
    let zip_path = meta
        .workspace_root
        .join("build")
        .join(format!("Remember-{version}-macos-arm64.zip"));

    // `ditto`, not `zip`/`tar` — it's the Apple-blessed way to archive a
    // signed `.app` without corrupting the code signature or resource forks.
    run_cmd(
        Command::new("ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(&app_path)
            .arg(&zip_path),
    )?;

    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(&zip_path)
        .output()
        .context("failed to spawn shasum")?;
    if !output.status.success() {
        bail!("shasum failed");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha256 = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("unexpected shasum output: {stdout}"))?;

    println!("{}", zip_path.display());
    println!("{sha256}");

    Ok(())
}

fn parse_version_flag(args: &[String]) -> Result<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--version=") {
            return Ok(value.to_string());
        }
        if arg == "--version" {
            return iter
                .next()
                .cloned()
                .ok_or_else(|| anyhow!("--version requires a value"));
        }
    }
    bail!("usage: cargo xtask package --version <x>")
}

/// Dot-separated numbers ("1", "1.4", "1.4.2"), the only shape both
/// `CFBundleShortVersionString` and `CFBundleVersion` accept. Same rule the
/// release workflow checks the tag against.
fn validate_version(version: &str) -> Result<()> {
    let valid = version
        .split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    if !valid {
        bail!("invalid version {version:?}: expected numbers separated by dots, e.g. 1.4");
    }
    Ok(())
}

/// `plist` (the checked-in Info.plist) with both version keys set to
/// `version`, so the app can show which release it is.
fn stamp_version(plist: &str, version: &str) -> Result<String> {
    validate_version(version)?;
    let plist = set_plist_string(plist, "CFBundleShortVersionString", version)?;
    set_plist_string(&plist, "CFBundleVersion", version)
}

/// Replaces the `<string>` value that follows `<key>{key}</key>`. Plain text
/// surgery, enough for the flat, hand-written plist this repo ships.
fn set_plist_string(plist: &str, key: &str, value: &str) -> Result<String> {
    let key_tag = format!("<key>{key}</key>");
    let missing = || anyhow!("Info.plist has no string value for {key}");
    let after_key = plist.find(&key_tag).ok_or_else(missing)? + key_tag.len();
    let rest = &plist[after_key..];
    let open = rest.find("<string>").ok_or_else(missing)?;
    // Only whitespace may sit between the key and its value; anything else
    // means the key's value isn't a string and `<string>` belongs to a
    // later key.
    if !rest[..open].trim().is_empty() {
        return Err(missing());
    }
    let start = after_key + open + "<string>".len();
    let end = start + plist[start..].find("</string>").ok_or_else(missing)?;
    Ok(format!("{}{value}{}", &plist[..start], &plist[end..]))
}

fn test() -> Result<()> {
    run_cmd(Command::new("cargo").args([
        "nextest",
        "run",
        "--workspace",
        "--features",
        "remember-core/testing",
    ]))
}

fn cov() -> Result<()> {
    run_cmd(Command::new("cargo").args([
        "llvm-cov",
        "nextest",
        "--package",
        "remember-core",
        "--package",
        "remember-sync",
        "--package",
        "remember-ffi",
        "--features",
        "remember-core/testing",
        "--ignore-filename-regex",
        r"crates/ffi/src/lib\.rs",
        "--fail-under-lines",
        "100",
        "--html",
        "--open",
    ]))
}

fn ci() -> Result<()> {
    run_cmd(Command::new("cargo").args(["fmt", "--check"]))?;
    run_cmd(Command::new("cargo").args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--features",
        "remember-core/testing",
        "--",
        "-D",
        "warnings",
    ]))?;
    test()?;
    cov()?;
    Ok(())
}

const PG_CONTAINER: &str = "remember-pgtest";
const PG_IMAGE: &str = "docker.io/library/postgres:17";
const PG_PORT: u16 = 54329;

/// Runs `crates/sql-tests` against `supabase/schema.sql` in a throwaway
/// Postgres container (Apple `container`): applies the Supabase shim, the
/// schema twice (to prove it re-runs cleanly) and the lockdown, runs the
/// ignored-by-default tests, and removes the container whatever happens.
fn pgtest() -> Result<()> {
    require_macos("pgtest")?;
    let meta = cargo_metadata()?;
    let supabase = meta.workspace_root.join("supabase");

    // Left over from a run that was killed before it could clean up.
    stop_pg_container();
    run_cmd(Command::new("container").args([
        "run",
        "--detach",
        "--rm",
        "--name",
        PG_CONTAINER,
        "--env",
        "POSTGRES_PASSWORD=pgtest",
        "--publish",
        &format!("127.0.0.1:{PG_PORT}:5432"),
        PG_IMAGE,
    ]))?;
    let result = pgtest_in_container(&supabase);
    stop_pg_container();
    result
}

fn pgtest_in_container(supabase: &Path) -> Result<()> {
    wait_for_pg()?;
    for file in [
        "test/supabase_shim.sql",
        "schema.sql",
        "schema.sql",
        "lockdown.sql",
    ] {
        let sql = fs::read_to_string(supabase.join(file))
            .with_context(|| format!("reading supabase/{file}"))?;
        run_cmd(Command::new("container").args([
            "exec",
            PG_CONTAINER,
            "psql",
            "--username=postgres",
            "--quiet",
            "--command",
            &sql,
        ]))
        .with_context(|| format!("applying supabase/{file}"))?;
    }

    run_cmd(
        Command::new("cargo")
            .args([
                "nextest",
                "run",
                "--package",
                "remember-sql-tests",
                "--run-ignored",
                "only",
            ])
            .env(
                "REMEMBER_SYNC_PG_URL",
                format!("postgres://postgres:pgtest@127.0.0.1:{PG_PORT}/postgres"),
            ),
    )
}

/// Polls over TCP, not the socket: the image's first-boot init runs a
/// temporary socket-only server that would pass a socket check before the
/// real one is up.
fn wait_for_pg() -> Result<()> {
    for _ in 0..60 {
        let ready = Command::new("container")
            .args([
                "exec",
                PG_CONTAINER,
                "pg_isready",
                "--host=127.0.0.1",
                "--username=postgres",
            ])
            .output()
            .is_ok_and(|o| o.status.success());
        if ready {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    bail!("Postgres in container `{PG_CONTAINER}` wasn't ready after 30s")
}

/// Best effort: fails harmlessly when there's nothing to stop. `--rm` on
/// `container run` deletes it once stopped.
fn stop_pg_container() {
    let _ = Command::new("container")
        .args(["stop", PG_CONTAINER])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &str = "<dict>
  <key>CFBundleVersion</key>           <string>0</string>
  <key>CFBundleShortVersionString</key><string>dev</string>
  <key>LSUIElement</key>               <true/>
  <key>CFBundleName</key>              <string>Remember</string>
</dict>";

    #[test]
    fn stamping_sets_both_version_keys_and_nothing_else() {
        let stamped = stamp_version(PLIST, "1.4").unwrap();
        assert_eq!(
            stamped,
            PLIST
                .replace("<string>0</string>", "<string>1.4</string>")
                .replace("<string>dev</string>", "<string>1.4</string>")
        );
    }

    #[test]
    fn the_checked_in_plist_can_be_stamped() {
        let plist = include_str!("../../../apple/Info-macOS.plist");
        let stamped = stamp_version(plist, "2.0.1").unwrap();
        assert_eq!(stamped.matches("<string>2.0.1</string>").count(), 2);
    }

    #[test]
    fn a_version_that_is_not_dot_separated_numbers_is_rejected() {
        for bad in ["", "v1.4", "1..4", "1.4.", "1.4-beta", "1.4</string>"] {
            assert!(stamp_version(PLIST, bad).is_err(), "{bad:?} was accepted");
        }
        assert!(validate_version("12.0.3").is_ok());
    }

    #[test]
    fn a_missing_key_is_an_error() {
        let err = set_plist_string(PLIST, "CFBundleIdentifier", "x").unwrap_err();
        assert!(err.to_string().contains("CFBundleIdentifier"));
    }

    #[test]
    fn a_key_whose_value_is_not_a_string_is_an_error_not_the_next_keys_value() {
        let err = set_plist_string(PLIST, "LSUIElement", "x").unwrap_err();
        assert!(err.to_string().contains("LSUIElement"));
    }

    #[test]
    fn an_unterminated_string_is_an_error() {
        let plist = "<key>CFBundleVersion</key><string>1";
        assert!(set_plist_string(plist, "CFBundleVersion", "2").is_err());
    }

    #[test]
    fn a_key_with_no_value_after_it_is_an_error() {
        let plist = "<key>CFBundleVersion</key>";
        assert!(set_plist_string(plist, "CFBundleVersion", "2").is_err());
    }
}
