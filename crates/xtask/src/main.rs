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
        Some("test") => test(),
        Some("cov") => cov(),
        Some("ci") => ci(),
        Some(other) => bail!("unknown xtask command: {other}"),
        None => bail!("usage: cargo xtask <bindings|build|mac|run|sim|device|test|cov|ci>"),
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

    run_cmd(Command::new("cargo").args(["build", "-p", "todo-ffi", "--release"]))?;

    let lib_path = meta
        .target_directory
        .join("release")
        .join(format!("libtodo_ffi.{}", dylib_extension()));

    let out_dir = meta.target_directory.join("uniffi-bindgen-out");
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    run_cmd(
        Command::new("cargo").args([
            "run",
            "-p",
            "todo-ffi",
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
        &out_dir.join("todo_ffi.swift"),
        &swift_dir.join("Sources/TodoKit/todo_ffi.swift"),
    )?;
    place_generated_file(
        &out_dir.join("todo_ffiFFI.h"),
        &swift_dir.join("Sources/TodoFFI/todo_ffiFFI.h"),
    )?;
    place_generated_file(
        &out_dir.join("todo_ffiFFI.modulemap"),
        &swift_dir.join("Sources/TodoFFI/module.modulemap"),
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

/// Build the todo-ffi staticlib for one Apple target triple and copy it into
/// the SwiftPM tree. There is exactly one staticlib path, overwritten with
/// whichever target was built last — no XCFramework, no `lipo`.
fn build(args: &[String]) -> Result<()> {
    let target = parse_target_flag(args)?;
    let meta = cargo_metadata()?;

    run_cmd(Command::new("cargo").args([
        "build",
        "-p",
        "todo-ffi",
        "--release",
        "--target",
        &target,
    ]))?;

    let lib_path = meta
        .target_directory
        .join(&target)
        .join("release")
        .join("libtodo_ffi.a");
    let dest = meta
        .workspace_root
        .join("swift/Sources/TodoFFI/lib/libtodo_ffi.a");
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

/// bindings + build + `swift build` + assemble `Todo.app` + ad-hoc sign.
fn mac() -> Result<()> {
    require_macos("mac")?;

    let meta = cargo_metadata()?;

    bindings()?;
    build(&["--target".to_string(), "aarch64-apple-darwin".to_string()])?;

    // Scoped to the TodoMac product: `swift build` otherwise builds every
    // target in the package, including TodoApp — the iOS entry point, whose
    // source doesn't exist until Phase 3 — and fails on its empty target.
    run_cmd(
        Command::new("swift")
            .args(["build", "-c", "release", "--product", "TodoMac"])
            .current_dir(meta.workspace_root.join("swift")),
    )?;

    assemble_app(&meta)?;

    run_cmd(
        Command::new("codesign").args([
            "--force",
            "--sign",
            "-",
            meta.workspace_root
                .join("build/Todo.app")
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 workspace root"))?,
        ]),
    )?;

    Ok(())
}

fn assemble_app(meta: &Metadata) -> Result<()> {
    let app = meta.workspace_root.join("build/Todo.app/Contents");
    let macos_dir = app.join("MacOS");
    let resources_dir = app.join("Resources");
    fs::create_dir_all(&macos_dir).with_context(|| format!("creating {}", macos_dir.display()))?;
    fs::create_dir_all(&resources_dir)
        .with_context(|| format!("creating {}", resources_dir.display()))?;

    fs::copy(
        meta.workspace_root.join("apple/Info-macOS.plist"),
        app.join("Info.plist"),
    )
    .context("copying Info-macOS.plist")?;

    fs::copy(
        meta.workspace_root.join("swift/.build/release/TodoMac"),
        macos_dir.join("Todo"),
    )
    .context("copying TodoMac executable")?;

    Ok(())
}

/// `mac`, then open the assembled bundle.
fn run() -> Result<()> {
    require_macos("run")?;
    let meta = cargo_metadata()?;
    mac()?;
    run_cmd(Command::new("open").arg(meta.workspace_root.join("build/Todo.app")))?;
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

fn test() -> Result<()> {
    run_cmd(Command::new("cargo").args([
        "nextest",
        "run",
        "--workspace",
        "--features",
        "todo-core/testing",
    ]))
}

fn cov() -> Result<()> {
    run_cmd(Command::new("cargo").args([
        "llvm-cov",
        "nextest",
        "--package",
        "todo-core",
        "--package",
        "todo-sync",
        "--package",
        "todo-ffi",
        "--package",
        "todo-cli",
        "--features",
        "todo-core/testing",
        "--ignore-filename-regex",
        r"(crates/ffi/src/lib\.rs|crates/cli/src/main\.rs)",
        "--fail-under-lines",
        "99",
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
        "todo-core/testing",
        "--",
        "-D",
        "warnings",
    ]))?;
    test()?;
    cov()?;
    Ok(())
}
