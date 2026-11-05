//! Asks the real program what it offers, and checks the guides say the same.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const ACTIONS: &str = include_str!("../docs/ACTIONS.md");
const MCP: &str = include_str!("../docs/MCP.md");
const README: &str = include_str!("../README.md");

/// Sends messages to the MCP server and returns its answers, in order.
fn ask(messages: &[Value]) -> Vec<Value> {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pyroporch"))
        .arg("mcp")
        .env("PYROPORCH_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let replies = messages
        .iter()
        .map(|message| {
            writeln!(stdin, "{message}").unwrap();
            stdin.flush().unwrap();
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        })
        .collect();
    drop(stdin);
    child.wait().unwrap();
    replies
}

#[test]
fn every_tool_is_in_the_assistant_guide_and_every_tool_named_there_exists() {
    let replies = ask(&[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})]);
    let tools: Vec<String> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap().to_string()).collect();
    assert!(tools.len() > 15, "{tools:?}");
    for tool in &tools {
        assert!(MCP.contains(&format!("`{tool}`")), "{tool} isn't in MCP.md");
    }
    // The first column of the tool tables, in both directions.
    for line in MCP.lines().filter(|line| line.starts_with("| `")) {
        let names = line.split('|').nth(1).unwrap();
        for name in names.split('`').skip(1).step_by(2) {
            assert!(tools.iter().any(|tool| tool == name), "MCP.md lists {name}, which doesn't exist");
        }
    }
}

#[test]
fn every_action_and_its_settings_are_in_the_actions_guide() {
    let replies = ask(&[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_actions", "arguments": {}}})]);
    let plugins = replies[0]["result"]["structuredContent"]["plugins"].as_array().unwrap().clone();
    assert!(plugins.len() >= 5, "{plugins:?}");
    for plugin in plugins {
        for action in plugin["actions"].as_array().unwrap() {
            let name = action["action"].as_str().unwrap();
            assert!(ACTIONS.contains(&format!("`{name}`")), "{name} isn't in ACTIONS.md");
            for field in action["fields"].as_array().unwrap() {
                let key = field["key"].as_str().unwrap();
                assert!(ACTIONS.contains(&format!("`{key}`")), "{name}'s {key} isn't in ACTIONS.md");
                for option in field["kind"]["options"].as_array().into_iter().flatten() {
                    let option = option.as_str().unwrap();
                    assert!(ACTIONS.contains(&format!("`{option}`")), "{name}'s {key} option {option} isn't in ACTIONS.md");
                }
            }
        }
    }
}

#[test]
fn every_command_is_in_the_readme() {
    let out = Command::new(env!("CARGO_BIN_EXE_pyroporch")).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout).into_owned();
    let commands: Vec<&str> = help
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pyroporch"))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter(|word| *word == "mcp" || word.starts_with("--"))
        .collect();
    assert!(commands.len() >= 6, "{help}");
    for command in commands {
        assert!(README.contains(&format!("`pyroporch {command}")), "{command} isn't in the README");
    }
}

#[test]
fn links_between_the_guides_lead_somewhere() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let guides = ["README.md", "docs/ACTIONS.md", "docs/CONFIG.md", "docs/MCP.md", "docs/SECURITY.md", "plugins/README.md"];
    for guide in guides {
        let path = root.join(guide);
        let text = std::fs::read_to_string(&path).unwrap();
        for (_, rest) in text.match_indices("](").map(|(at, _)| ((), &text[at + 2..])) {
            let target = rest.split(')').next().unwrap();
            if target.starts_with("http") {
                continue;
            }
            let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
            let linked = if file.is_empty() { path.clone() } else { path.parent().unwrap().join(file) };
            let contents = std::fs::read_to_string(&linked).unwrap_or_else(|_| panic!("{guide} links to {target}, which isn't there"));
            if !anchor.is_empty() {
                let found = contents.lines().filter(|line| line.starts_with('#')).any(|heading| slug(heading) == anchor);
                assert!(found, "{guide} links to {target}, but there's no such heading");
            }
        }
    }
}

/// The anchor a heading gets on GitHub.
fn slug(heading: &str) -> String {
    heading
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}
