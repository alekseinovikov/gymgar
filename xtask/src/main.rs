//! Build tasks, run as `cargo xtask <task>`:
//!
//! - `ci`: fmt check, clippy with `-D warnings`, tests, layering. The gate before finishing a task.
//! - `layers`: check the crate dependency layering (`layers.rs`).

mod layers;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("ci") => ci(),
        Some("layers") => check_layers(),
        _ => {
            eprintln!("usage: cargo xtask <ci|layers>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The cargo that runs us (`cargo xtask` sets `CARGO`), so the same toolchain checks the tree.
fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run_cargo(args: &[&str]) -> Result<(), String> {
    let line = args.join(" ");
    eprintln!("$ cargo {line}");
    let status = cargo().args(args).status().map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("`cargo {line}` failed ({status})")) }
}

fn ci() -> Result<(), String> {
    run_cargo(&["fmt", "--all", "--check"])?;
    run_cargo(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    run_cargo(&["test", "--workspace"])?;
    check_layers()
}

fn check_layers() -> Result<(), String> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata output: {e}"))?;
    let packages = layers::from_metadata(&meta)?;
    let errors = layers::check(&packages);
    if errors.is_empty() {
        eprintln!("layers: {} crates OK", packages.len());
        return Ok(());
    }
    for err in &errors {
        eprintln!("layers: {err}");
    }
    Err(format!("{} layering violation(s)", errors.len()))
}
