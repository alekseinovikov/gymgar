//! A command line the CLI cannot understand prints the usage to stderr and exits with 2, touching nothing else.

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gymgar-cli")).args(args).output().expect("run gymgar-cli")
}

#[test]
fn usage_goes_to_stderr_with_exit_code_2() {
    for args in [&[][..], &["--help"], &["-h"], &["help"], &["bogus"], &["mcp", "extra"]] {
        let output = cli(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        // Stdout is the MCP channel: it must stay empty.
        assert!(output.stdout.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&output.stdout));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Usage: gymgar-cli <command>") && stderr.contains("mcp"), "{args:?}: {stderr}");
    }
}

#[test]
fn an_unknown_command_is_named() {
    let stderr = String::from_utf8_lossy(&cli(&["bogus"]).stderr).into_owned();
    assert!(stderr.contains("unknown command `bogus`"), "{stderr}");
}
