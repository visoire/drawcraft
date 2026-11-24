//! `xtask` - workspace automation (`cargo xtask <command>`).

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod branding;
mod layers;
mod panic_hunt;
mod web;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <command>

commands:
  layers       verify L0-L6 crate dependency boundaries
  branding     regenerate the original app icon set (PWA, desktop, UI)
  panic-hunt   verify no unwrap/expect/panic!/todo!/unimplemented!/unreachable! in non-test code
  ci           fmt --check, clippy (native + wasm32 web), layers, panic-hunt, test --workspace
  web          serve the WASM/PWA app on localhost (requires `trunk`)
  dev          serve on your LAN and print the URL to open on an iPad (add --https for the full PWA)
  web-build    build the WASM/PWA bundle into apps/drawcraft-web/dist and verify it (requires `trunk`)";

fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Run a command with inherited stdio, failing on a non-zero exit status.
fn run_cmd(program: impl AsRef<std::ffi::OsStr>, args: &[&str], dir: &Path) -> Result<(), String> {
    let program = program.as_ref();
    println!("==> {} {}", program.to_string_lossy(), args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("failed to start {}: {e}", program.to_string_lossy()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {} exited with {status}",
            program.to_string_lossy(),
            args.join(" ")
        ))
    }
}

fn ci(root: &Path) -> Result<(), String> {
    type Step<'a> = (&'a str, Box<dyn Fn() -> Result<(), String> + 'a>);
    let steps: Vec<Step<'_>> = vec![
        (
            "fmt --check",
            Box::new(|| run_cmd(cargo(), &["fmt", "--all", "--", "--check"], root)),
        ),
        (
            "clippy --all-targets",
            Box::new(|| {
                run_cmd(
                    cargo(),
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                    root,
                )
            }),
        ),
        (
            "clippy wasm32 (web)",
            Box::new(|| {
                run_cmd(
                    cargo(),
                    &[
                        "clippy",
                        "-p",
                        "drawcraft-web",
                        "--target",
                        "wasm32-unknown-unknown",
                        "--",
                        "-D",
                        "warnings",
                    ],
                    root,
                )
            }),
        ),
        ("layers", Box::new(|| layers::run(root))),
        ("panic-hunt", Box::new(|| panic_hunt::run(root))),
        (
            "test --workspace",
            Box::new(|| run_cmd(cargo(), &["test", "--workspace"], root)),
        ),
    ];
    let mut failed = Vec::new();
    let mut summary = Vec::new();
    for (name, step) in &steps {
        match step() {
            Ok(()) => summary.push(format!("  ok    {name}")),
            Err(e) => {
                eprintln!("ci: {name} failed: {e}");
                summary.push(format!("  FAIL  {name}"));
                failed.push(*name);
            }
        }
    }
    println!("\nci summary:\n{}", summary.join("\n"));
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("ci failed: {}", failed.join(", ")))
    }
}

fn main() -> ExitCode {
    let root = workspace_root();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("layers") => layers::run(&root),
        Some("branding") => branding::run(&root),
        Some("panic-hunt") => panic_hunt::run(&root),
        Some("ci") => ci(&root),
        Some("web") => web::serve_local(&root),
        Some("dev") => web::serve_lan(&root, args.iter().any(|a| a == "--https")),
        Some("web-build") => web::build(&root),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}
