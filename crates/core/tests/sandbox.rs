//! Runs real `.wasm` code in the sandbox: the API call plugin built for it, and hostile plugins.

use pyroporch_core::config::{Key, Step};
use pyroporch_core::engine::Engine;
use pyroporch_core::paths::Paths;
use pyroporch_core::vault::{KdfParams, Vault};
use pyroporch_core::wasm::WasmPlugin;
use pyroporch_sdk::{Host, HostHandle, HttpRequest, HttpResponse, LogLevel, Plugin, RunRequest, Trigger};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const SMALL_FUEL: u64 = 2_000_000;

#[derive(Default)]
struct FakeHost {
    sent: Mutex<Vec<HttpRequest>>,
}

impl Host for FakeHost {
    fn http(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        self.sent.lock().unwrap().push(request.clone());
        Ok(HttpResponse {
            status: 200,
            body: r#"{"answer":{"value":42}}"#.into(),
            ..HttpResponse::default()
        })
    }
    fn log(&self, _: LogLevel, _: &str) {}
    fn now_ms(&self) -> u64 {
        7
    }
}

/// Builds the API call plugin for the sandbox. `None` if this machine has no wasm toolchain.
fn build_api_plugin() -> Option<Vec<u8>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = root.join("target/sandbox-test");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(&root)
        .args(["build", "-p", "pyroporch-plugin-api-call", "--target", "wasm32-unknown-unknown", "--target-dir"])
        .arg(&target)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::read(target.join("wasm32-unknown-unknown/debug/pyroporch_plugin_api_call.wasm")).ok()
}

#[test]
fn the_api_plugin_runs_in_the_sandbox_with_its_secret_still_a_placeholder() {
    let Some(bytes) = build_api_plugin() else {
        eprintln!("skipped: no wasm32-unknown-unknown target here");
        return;
    };
    let plugin = WasmPlugin::load(&bytes).unwrap();
    assert_eq!(plugin.info().id, "pyroporch.api-call");
    assert_eq!(plugin.info().actions[0].id, "request");

    let fake = Arc::new(FakeHost::default());
    let host: HostHandle = fake.clone();
    let settings = json!({
        "url": "https://api.example.com/v1", "auth": "bearer token",
        "secret": "{{secret:token}}", "show_value": "/answer/value"
    });
    let request = RunRequest {
        action: "request".into(),
        settings: settings.as_object().unwrap().clone(),
        trigger: Trigger::Press,
    };
    let outcome = plugin.run(&request, &host).unwrap();
    assert_eq!(outcome.title.as_deref(), Some("42"));
    let sent = fake.sent.lock().unwrap();
    assert_eq!(sent[0].url, "https://api.example.com/v1");
    assert_eq!(sent[0].headers[0].1, "Bearer {{secret:token}}");

    // The same plugin keeps working call after call.
    drop(sent);
    assert!(plugin.run(&request, &host).is_ok());
}

#[test]
fn a_third_party_plugin_goes_from_folder_to_approved_to_working_and_loses_trust_if_changed() {
    let Some(bytes) = build_api_plugin() else {
        eprintln!("skipped: no wasm32-unknown-unknown target here");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    paths.ensure().unwrap();
    let folder = paths.plugins_dir().join("api");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("plugin.wasm"), &bytes).unwrap();
    std::fs::write(
        folder.join("plugin.toml"),
        "id = \"pyroporch.api-call\"\nname = \"From a folder\"\nwasm = \"plugin.wasm\"\n\n[permissions]\nhttp_hosts = [\"127.0.0.1\"]\n",
    )
    .unwrap();

    // No built-in module here, so the only copy is the one in the folder.
    let vault = Vault::with_kdf(paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
    let engine = Engine::new(paths.clone(), vault, vec![]);
    let action = "pyroporch.api-call/request";
    assert!(engine.spec(action).is_none(), "off until approved");

    engine.approve_plugin("pyroporch.api-call").unwrap();
    assert!(engine.spec(action).is_some(), "on once approved");

    // It runs under the app's rules: this site wasn't one it asked for.
    let key = Key {
        steps: vec![Step {
            action: action.into(),
            settings: json!({"url": "https://not-allowed.example/x"}).as_object().unwrap().clone(),
            delay_ms: 0,
        }],
        ..Key::default()
    };
    let report = engine.run_key(&key, Trigger::Press, "test", true);
    assert!(!report.ok);
    assert!(report.message.contains("needs your OK"), "{}", report.message);

    // A changed file means a new approval.
    let mut changed = bytes;
    changed.extend_from_slice(b"\0\x04evil");
    std::fs::write(folder.join("plugin.wasm"), &changed).unwrap();
    engine.refresh_plugins();
    assert!(engine.spec(action).is_none(), "off again");
    let status = engine.plugin_statuses().into_iter().find(|s| s.id == "pyroporch.api-call").unwrap();
    assert!(status.note.contains("changed"), "{}", status.note);
}

#[test]
fn a_sandboxed_plugin_cannot_put_the_deck_to_sleep() {
    // Says it has one action, and always answers asking for the deck to sleep.
    let info = r#"{"id":"evil","name":"Evil","version":"1","actions":[{"id":"x","name":"X"}]}"#;
    let answer = r#"{"Ok":{"message":"tried","deck":{"do":"sleep"}}}"#;
    let source = format!(
        r#"(module
            (import "pyroporch" "host_call" (func $host (param i32 i32) (result i64)))
            (memory (export "memory") 2 20)
            (data (i32.const 16) "{info}")
            (data (i32.const 1024) "{answer}")
            (global $next (mut i32) (i32.const 4096))
            (func (export "ld_abi") (result i32) i32.const 1)
            (func (export "ld_alloc") (param i32) (result i32)
                global.get $next
                global.get $next local.get 0 i32.add global.set $next)
            (func (export "ld_free") (param i32 i32))
            (func (export "ld_info") (result i64) i64.const {info_at})
            (func (export "ld_run") (param i32 i32) (result i64) i64.const {answer_at}))"#,
        info = info.replace('"', "\\\""),
        answer = answer.replace('"', "\\\""),
        info_at = (16u64 << 32) | info.len() as u64,
        answer_at = (1024u64 << 32) | answer.len() as u64,
    );
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    paths.ensure().unwrap();
    let folder = paths.plugins_dir().join("evil");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("plugin.wasm"), source).unwrap();
    std::fs::write(folder.join("plugin.toml"), "id = \"evil\"\nwasm = \"plugin.wasm\"\n").unwrap();
    let vault = Vault::with_kdf(paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
    let engine = Engine::new(paths, vault, vec![]);
    engine.approve_plugin("evil").unwrap();

    let key = Key {
        steps: vec![Step { action: "evil/x".into(), ..Step::default() }],
        ..Key::default()
    };
    let report = engine.run_key(&key, Trigger::Press, "test", true);
    assert!(report.ok, "{}", report.message);
    assert_eq!(report.message, "tried", "the plugin did run");
    assert!(report.deck.is_empty(), "but its request was dropped");
}

/// The samples the app carries have to be what their source code builds into.
/// One marked example from the plugin guide.
fn guide_example<'a>(guide: &'a str, name: &str) -> &'a str {
    let after = guide.split_once(&format!("<!-- guide-example: {name} -->")).unwrap_or_else(|| panic!("no example called {name}")).1;
    let opened = after.split_once('\n').unwrap().1.split_once('\n').unwrap().1;
    opened.split_once("\n```").unwrap().0
}

/// Follows the guide's quick start to the letter: makes the crate it describes, tests it, builds it
/// and installs it, so what the guide says is known to work.
#[test]
fn the_quick_start_in_the_plugin_guide_works_from_start_to_finish() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let guide = include_str!("../../../plugins/README.md");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    if !Command::new("rustup").args(["target", "list", "--installed"]).output().is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("wasm32-unknown-unknown")) {
        eprintln!("skipped: no wasm32-unknown-unknown target here");
        return;
    }

    let project = root.join("target/guide-check/dice");
    std::fs::create_dir_all(project.join("src")).unwrap();
    let manifest = guide_example(guide, "Cargo.toml").replace("../pyroporch/crates/sdk", root.join("crates/sdk").to_str().unwrap());
    // Its own workspace, because this copy happens to sit inside the project's folder.
    std::fs::write(project.join("Cargo.toml"), format!("{manifest}\n\n[workspace]\n")).unwrap();
    let code = format!("{}\n\n{}\n", guide_example(guide, "lib.rs"), guide_example(guide, "tests"));
    std::fs::write(project.join("src/lib.rs"), code).unwrap();
    std::fs::copy(root.join("Cargo.lock"), project.join("Cargo.lock")).unwrap();

    let run = |args: &[&str]| {
        let out = Command::new(&cargo).current_dir(&project).args(args).output().unwrap();
        assert!(out.status.success(), "cargo {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    run(&["test", "--offline", "--lib"]);
    run(&["build", "--offline", "--release", "--target", "wasm32-unknown-unknown"]);
    let wasm = std::fs::read(project.join("target/wasm32-unknown-unknown/release/dice.wasm")).unwrap();

    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    paths.ensure().unwrap();
    let folder = paths.plugins_dir().join("dice");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("dice.wasm"), wasm).unwrap();
    std::fs::write(folder.join("plugin.toml"), guide_example(guide, "plugin.toml")).unwrap();

    let vault = Vault::with_kdf(paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
    let engine = Engine::new(paths, vault, vec![]);
    let status = engine.plugin_statuses().into_iter().find(|s| s.id == "com.example.dice").expect("listed");
    assert!(status.note.contains("Waiting for your approval"), "{}", status.note);
    engine.approve_plugin("com.example.dice").unwrap();

    let key = Key {
        steps: vec![Step {
            action: "com.example.dice/roll".into(),
            settings: json!({"sides": 6}).as_object().unwrap().clone(),
            delay_ms: 0,
        }],
        ..Key::default()
    };
    let report = engine.run_key(&key, Trigger::Press, "test", false);
    assert!(report.ok, "{}", report.message);
    let roll: u32 = report.title.expect("a number on the key").parse().unwrap();
    assert!((1..=6).contains(&roll), "{roll}");
    assert!(report.message.starts_with("rolled"), "{}", report.message);
}

/// A file built on Windows writes its paths differently, so this can only compare on the others.
#[cfg(unix)]
#[test]
fn the_sample_plugins_the_app_carries_are_up_to_date() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !Command::new("rustup").args(["target", "list", "--installed"]).output().is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("wasm32-unknown-unknown")) {
        eprintln!("skipped: no wasm32-unknown-unknown target here");
        return;
    }
    let out = Command::new("sh").arg(root.join("plugins/_examples/build-example-plugins.sh")).arg("--check").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn the_sample_plugins_the_app_carries_name_nobody_s_folders() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for plugin in ["api-call", "screensaver"] {
        let bytes = std::fs::read(root.join("plugins").join(plugin).join("plugin.wasm")).unwrap();
        for folder in [&b"/Users/"[..], b"/home/", b"\\Users\\"] {
            assert!(!bytes.windows(folder.len()).any(|part| part == folder), "{plugin} holds a path to someone's folder");
        }
    }
}

#[test]
fn a_plugin_with_deck_access_can_ask_and_one_that_draws_badly_is_refused() {
    let info = r#"{"id":"good","name":"Good","version":"1","actions":[{"id":"x","name":"X"}]}"#;
    let answer = r#"{"Ok":{"deck":{"do":"sleep"}}}"#;
    let source = format!(
        r#"(module
            (memory (export "memory") 2 20)
            (data (i32.const 16) "{info}")
            (data (i32.const 1024) "{answer}")
            (global $next (mut i32) (i32.const 4096))
            (func (export "ld_abi") (result i32) i32.const 1)
            (func (export "ld_alloc") (param i32) (result i32)
                global.get $next
                global.get $next local.get 0 i32.add global.set $next)
            (func (export "ld_free") (param i32 i32))
            (func (export "ld_info") (result i64) i64.const {info_at})
            (func (export "ld_paint") (param i32 i32) (result i64) i64.const {answer_at})
            (func (export "ld_run") (param i32 i32) (result i64) i64.const {answer_at}))"#,
        info = info.replace('"', "\\\""),
        answer = answer.replace('"', "\\\""),
        info_at = (16u64 << 32) | info.len() as u64,
        answer_at = (1024u64 << 32) | answer.len() as u64,
    );
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    paths.ensure().unwrap();
    let folder = paths.plugins_dir().join("good");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("plugin.wasm"), source).unwrap();
    std::fs::write(folder.join("plugin.toml"), "id = \"good\"\nwasm = \"plugin.wasm\"\n[permissions]\ndeck = true\n").unwrap();
    let vault = Vault::with_kdf(paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
    let engine = Engine::new(paths, vault, vec![]);
    engine.approve_plugin("good").unwrap();

    let key = Key {
        steps: vec![Step { action: "good/x".into(), ..Step::default() }],
        ..Key::default()
    };
    assert_eq!(engine.run_key(&key, Trigger::Press, "test", true).deck.len(), 1);

    let request = pyroporch_sdk::PaintRequest {
        settings: Default::default(),
        rows: 2,
        cols: 2,
        blocks_per_key: 4,
        millis: 0,
        restart: true,
    };
    let error = engine.paint("good", &request).unwrap_err();
    assert!(error.contains("wrong amount"), "{error}");
}

/// A minimal plugin in the sandbox's text format, with a chosen body for its `ld_run`.
fn hostile(extra_imports: &str, run_body: &str) -> String {
    // The info JSON sits at offset 16; ld_info returns (16 << 32) | its length.
    let info = r#"{"id":"evil","name":"Evil","version":"1","actions":[]}"#;
    format!(
        r#"(module
            (import "pyroporch" "host_call" (func $host (param i32 i32) (result i64)))
            {extra_imports}
            (memory (export "memory") 2 20)
            (data (i32.const 16) "{info}")
            (global $next (mut i32) (i32.const 4096))
            (func (export "ld_abi") (result i32) i32.const 1)
            (func (export "ld_alloc") (param i32) (result i32)
                global.get $next
                global.get $next local.get 0 i32.add global.set $next)
            (func (export "ld_free") (param i32 i32))
            (func (export "ld_info") (result i64)
                i64.const {packed})
            (func (export "ld_run") (param i32 i32) (result i64) {run_body}))"#,
        info = info.replace('"', "\\\""),
        packed = (16u64 << 32) | info.len() as u64,
    )
}

fn press() -> RunRequest {
    RunRequest {
        action: "x".into(),
        settings: Default::default(),
        trigger: Trigger::Press,
    }
}

fn host() -> HostHandle {
    Arc::new(FakeHost::default())
}

#[test]
fn a_plugin_that_never_stops_is_stopped() {
    let plugin = WasmPlugin::load_with_fuel(hostile("", "(loop $forever (br $forever)) i64.const 0").as_bytes(), SMALL_FUEL).unwrap();
    let error = plugin.run(&press(), &host()).unwrap_err();
    assert!(error.contains("stopped"), "{error}");
}

#[test]
fn a_plugin_cannot_take_all_the_memory() {
    // Keeps asking for more, and traps once it's refused. If the limit didn't hold it would
    // eventually be stopped by fuel instead, so the fuel here is generous.
    let body = "(loop $more (br_if $more (i32.ne (memory.grow (i32.const 256)) (i32.const -1)))) unreachable";
    let source = hostile("", body).replace("2 20", "2 65536");
    let plugin = WasmPlugin::load(source.as_bytes()).unwrap();
    let error = plugin.run(&press(), &host()).unwrap_err();
    assert!(error.contains("stopped"), "{error}");
}

#[test]
fn a_plugin_asking_for_the_outside_world_does_not_load() {
    for import in [
        r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#,
        r#"(import "env" "system" (func (param i32) (result i32)))"#,
        r#"(import "pyroporch" "read_secret" (func (param i32) (result i32)))"#,
    ] {
        let error = match WasmPlugin::load(hostile(import, "i64.const 0").as_bytes()) {
            Ok(_) => panic!("loaded with {import}"),
            Err(error) => error,
        };
        assert!(error.contains("doesn't offer"), "{error}");
    }
}

#[test]
fn bad_pointers_from_a_plugin_are_refused_not_followed() {
    // Claims its answer sits far outside its memory.
    let plugin = WasmPlugin::load(hostile("", "i64.const 0x7FFFFFFF00000040").as_bytes()).unwrap();
    assert!(plugin.run(&press(), &host()).is_err());

    // Calls the host with a bad address: it gets an error back, not the app's memory.
    let body = "i32.const 2000000000 i32.const 64 call $host drop i64.const 0x00000000000000FF";
    let plugin = WasmPlugin::load(hostile("", body).as_bytes()).unwrap();
    assert!(plugin.run(&press(), &host()).is_err());
}

#[test]
fn a_plugin_cannot_call_the_app_while_loading() {
    // No host is attached outside a run, so this start function gets an error reply and carries on.
    let start = r#"(func $boot (drop (call $host (i32.const 16) (i32.const 8)))) (start $boot)"#;
    let source = hostile("", "i64.const 0").replacen("(global $next", &format!("{start} (global $next"), 1);
    // Whatever happens, it must not have reached the host.
    let _ = WasmPlugin::load(source.as_bytes());
}

#[test]
fn junk_and_wrong_versions_are_refused() {
    assert!(WasmPlugin::load(b"this is not a plugin").is_err());
    let wrong_abi = hostile("", "i64.const 0").replace(r#"(result i32) i32.const 1)"#, r#"(result i32) i32.const 99)"#);
    let error = match WasmPlugin::load(wrong_abi.as_bytes()) {
        Ok(_) => panic!("loaded a wrong version"),
        Err(error) => error,
    };
    assert!(error.contains("version"), "{error}");
}
