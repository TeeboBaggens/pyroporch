//! Talks to the real program over the real connection, the way an AI assistant would.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Client {
    fn start(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pyroporch"))
            .arg("mcp")
            .env("PYROPORCH_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdin, stdout, next_id: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    fn call(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()));
        (result["isError"].as_bool().unwrap(), parsed)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn an_assistant_adds_a_working_key_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let mut client = Client::start(home.path());

    let reply = client.request("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
    assert_eq!(reply["result"]["serverInfo"]["name"], "pyroporch");
    writeln!(client.stdin, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();

    let reply = client.request("server/discover", json!({}));
    assert_eq!(reply["result"]["supportedVersions"][0], "2026-07-28");

    // The API call sample and the Desktop actions are both there.
    let (failed, actions) = client.call("list_actions", json!({}));
    assert!(!failed);
    let text = actions.to_string();
    assert!(text.contains("pyroporch.api-call/request"), "{text}");
    assert!(text.contains("pyroporch.desktop/hotkey"), "{text}");

    // A credential in plain text never gets saved.
    let (failed, reason) = client.call("set_key", json!({
        "row": 2, "col": 0, "title": "Leak",
        "steps": [{"action": "pyroporch.api-call/request", "settings": {
            "url": "https://api.example.com/x",
            "headers": {"Authorization": "Bearer real-looking-token"}
        }}]
    }));
    assert!(failed, "{reason}");
    let saved = std::fs::read_to_string(home.path().join("profiles/default.toml")).unwrap();
    assert!(!saved.contains("real-looking-token"));
    assert!(!saved.contains("Leak"));

    // A reference is fine, and the person gets asked for what only they can decide.
    let (failed, secret) = client.call("request_secret", json!({
        "name": "svc_key", "hosts": ["api.example.com"], "reason": "For the status key"
    }));
    assert!(!failed, "{secret}");
    let reference = secret["reference"].as_str().unwrap().to_string();
    let (failed, reply) = client.call("set_key", json!({
        "row": 2, "col": 0, "title": "Status", "icon": "🌐",
        "steps": [{"action": "pyroporch.api-call/request", "settings": {
            "url": "https://api.example.com/x", "auth": "bearer token", "secret": reference
        }}]
    }));
    assert!(!failed, "{reply}");
    assert!(reply["needs_person"].to_string().contains("api.example.com"), "{reply}");

    // The change is in the same config file the app watches.
    let saved = std::fs::read_to_string(home.path().join("profiles/default.toml")).unwrap();
    assert!(saved.contains("Status"));
    assert!(saved.contains("{{secret:svc_key}}"));
    let pending = std::fs::read_to_string(home.path().join("pending.toml")).unwrap();
    assert!(pending.contains("svc_key"));
    assert!(pending.contains("api.example.com"));

    // Pressing it doesn't reach the site: nobody has allowed it yet.
    let (_, pressed) = client.call("press_key", json!({"row": 2, "col": 0}));
    assert_eq!(pressed["ok"], false, "{pressed}");
    assert!(pressed["message"].as_str().unwrap().contains("needs your OK"), "{pressed}");

    // A risky key can be written but not run.
    let (failed, reply) = client.call("set_key", json!({
        "row": 2, "col": 1, "title": "Script",
        "steps": [{"action": "pyroporch.desktop/run_command", "settings": {"program": "/usr/bin/true"}}]
    }));
    assert!(!failed, "{reply}");
    let (_, pressed) = client.call("press_key", json!({"row": 2, "col": 1}));
    assert_eq!(pressed["ok"], false, "{pressed}");
    assert!(pressed["message"].as_str().unwrap().contains("approval"), "{pressed}");
}

#[cfg(unix)]
#[test]
fn a_desktop_key_really_starts_a_program_once_the_person_approves_it() {
    let home = tempfile::tempdir().unwrap();
    let mut client = Client::start(home.path());
    let folder = home.path().join("marker");
    std::fs::create_dir(&folder).unwrap();
    let args = folder.join("it-ran").to_string_lossy().into_owned();
    let (failed, reply) = client.call("set_key", json!({
        "row": 2, "col": 1, "title": "Touch",
        "steps": [{"action": "pyroporch.desktop/run_command", "settings": {"program": "/usr/bin/touch", "args": args}}]
    }));
    assert!(!failed, "{reply}");
    let (_, pressed) = client.call("press_key", json!({"row": 2, "col": 1}));
    assert_eq!(pressed["ok"], false, "{pressed}");
    assert!(!folder.join("it-ran").exists(), "nothing happens before it's approved");

    // The person approves it in the app.
    let mut ws = pyroporch_core::config::Workspace::load(pyroporch_core::paths::Paths::at(home.path())).unwrap();
    ws.profiles.get_mut("default").unwrap().pages[0].key_at_mut(2, 1).unwrap().approve();
    ws.save_profile("default").unwrap();

    let (_, pressed) = client.call("press_key", json!({"row": 2, "col": 1}));
    assert_eq!(pressed["ok"], true, "{pressed}");
    for _ in 0..200 {
        if folder.join("it-ran").exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the program didn't run");
}

#[test]
fn the_program_says_what_it_can_do_and_refuses_nonsense() {
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_pyroporch")).args(args).output().unwrap();
    assert!(String::from_utf8_lossy(&run(&["--help"]).stdout).contains("mcp"));
    assert!(String::from_utf8_lossy(&run(&["--version"]).stdout).starts_with("pyroporch "));
    assert!(!run(&["--nonsense"]).status.success());
    assert!(!run(&["--screenshot"]).status.success());
}
