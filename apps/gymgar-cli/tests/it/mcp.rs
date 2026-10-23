//! `gymgar-cli mcp` over real pipes: the binary opens its database and answers the protocol on stdout.
//!
//! The home directory is a temporary one, so the test never touches the real data directory. Unix only: elsewhere
//! the data directory comes from the system, not from the environment.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn serves_the_two_tools_and_runs_commands_over_stdio() {
    let home = tempfile::tempdir().expect("a temporary home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gymgar-cli"))
        .arg("mcp")
        .env("HOME", home.path())
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start gymgar-cli mcp");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    // Reading happens on a thread so that a server that never answers fails the test instead of hanging it.
    let (lines, answers) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut exchange = |request: Value| -> Option<Value> {
        writeln!(stdin, "{request}").expect("write a request");
        stdin.flush().expect("flush");
        let id = request.get("id")?.clone();
        loop {
            let line = answers.recv_timeout(TIMEOUT).unwrap_or_else(|error| {
                let _ = child.kill();
                panic!("no answer to {request} ({error})")
            });
            let answer: Value = serde_json::from_str(&line).unwrap_or_else(|error| panic!("stdout must carry JSON only ({error}): {line}"));
            if answer.get("id") == Some(&id) {
                return Some(answer);
            }
        }
    };

    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } });
    let answer = exchange(init).expect("an answer");
    assert_eq!(answer["result"]["serverInfo"]["name"], "gymgar");
    assert_eq!(exchange(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), None);

    let tools = exchange(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).expect("an answer");
    let names: Vec<&str> = tools["result"]["tools"].as_array().expect("tools").iter().filter_map(|tool| tool["name"].as_str()).collect();
    assert_eq!(names, ["command_list", "command_run"]);

    let call =
        |id: u32, arguments: Value| json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": "command_run", "arguments": arguments } });
    let set = exchange(call(3, json!({ "id": "settings.setLanguage", "params": { "language": "ru" } }))).expect("an answer");
    assert_eq!(set["result"]["isError"], false, "{set}");
    let get = exchange(call(4, json!({ "id": "settings.get" }))).expect("an answer");
    assert_eq!(get["result"]["content"][0]["text"], "{\"language\":\"ru\"}", "{get}");
    // The headless CLI has no secret store, and agents cannot run this one anyway.
    let hidden = exchange(call(5, json!({ "id": "garmin.disconnect" }))).expect("an answer");
    assert_eq!(hidden["result"]["isError"], true, "{hidden}");
    let bad = exchange(call(6, json!({ "id": "settings.get", "extra": 1 }))).expect("an answer");
    assert_eq!(bad["error"]["code"], -32602, "{bad}");

    // The client goes away: the server ends cleanly.
    drop(stdin);
    let status = wait_for_exit(&mut child);
    assert!(status.success(), "{status}");
    assert!(contains_file(home.path(), "gymgar.sqlite3"), "the database should be created under the temporary home");
}

/// Waits for `child` to exit, but not for ever: a server that keeps running after its client has gone is killed, and the
/// test fails instead of hanging the run.
fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("check whether gymgar-cli has exited") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("gymgar-cli did not exit within {TIMEOUT:?} after its client went away");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn contains_file(dir: &Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() { contains_file(&path, name) } else { entry.file_name() == name }
    })
}
