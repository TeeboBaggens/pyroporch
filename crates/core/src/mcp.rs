//! The built-in MCP server. An AI assistant starts it with `pyroporch mcp` and talks over
//! stdin/stdout. It opens no network port, so nothing else can reach it.
//!
//! What it deliberately can't do: read a secret, store a secret, approve a site, approve a key or
//! approve a plugin. For those it can only ask, and the person answers in the app.

use serde_json::{Map, Value, json};
use std::io::{BufRead, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{BACK_SLOT, Key, MAX_FOLDER_DEPTH, Page, Preset, Profile, Step, Workspace};
use crate::engine::{Block, Engine};
use crate::paths::audit;
use crate::policy::{Grants, Pending};
use crate::security::{find_refs, plaintext_findings};
use crate::transfer::{MAX_IMAGE_BYTES, store_image};
use crate::vault::{normalize_hosts, valid_name};
use pyroporch_sdk::{DeckControl, Trigger};

/// Current revision first. Older ones are served for clients that open with `initialize`.
/// Protocol revisions checked against modelcontextprotocol.io on 2026-11-03.
const MODERN_VERSIONS: [&str; 1] = ["2026-07-28"];
const LEGACY_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// How long a key pressed by an assistant may take before the server gives up on it.
const PRESS_TIMEOUT: Duration = Duration::from_secs(60);
const LIST_TTL_MS: u64 = 60_000;

const INSTRUCTIONS: &str = "Pyro Porch is a button deck. Start with get_overview, then list_actions to see what a key can do \
and which settings each action takes. Use set_key to add keys, with is_folder for a folder and the folder argument to reach \
inside one. Never put a password, API key or token in a setting: call request_secret, ask the person to enter it in the app, \
then use the {{secret:name}} reference it gives you. You can't read secrets. Sites and keys that act on the computer need the \
person's approval in the app; the replies say when. Run check_config after making changes. To learn more, or to add new \
abilities with a plugin, call get_guide.";

/// The app's own guides, so an assistant can read them wherever the app is installed.
const GUIDES: [(&str, &str); 5] = [
    ("plugins", include_str!("../../../plugins/README.md")),
    ("actions", include_str!("../../../docs/ACTIONS.md")),
    ("config", include_str!("../../../docs/CONFIG.md")),
    ("assistant", include_str!("../../../docs/MCP.md")),
    ("security", include_str!("../../../docs/SECURITY.md")),
];

const THEMES: [&str; 2] = crate::config::THEMES;
const TRAY_ICONS: [&str; 3] = crate::config::TRAY_ICONS;
const SCALE_RANGE: std::ops::RangeInclusive<f64> = crate::config::SCALE_RANGE;

pub struct McpServer {
    ws: Workspace,
    engine: Arc<Engine>,
}

fn server_info() -> Value {
    json!({"name": "pyroporch", "version": env!("CARGO_PKG_VERSION")})
}

/// What a tool does to the person's setup, which clients can use when deciding what to confirm.
#[derive(Clone, Copy)]
enum Kind {
    /// Changes nothing.
    Reads,
    /// Only ever adds something, or asks the person.
    Adds,
    /// Can replace or remove something.
    Changes,
    /// Runs a key, which can act on the world outside the app.
    Presses,
}

fn tool(name: &str, title: &str, description: &str, kind: Kind, properties: Value, required: &[&str]) -> Value {
    let annotations = match kind {
        Kind::Reads => json!({"readOnlyHint": true, "openWorldHint": false}),
        Kind::Adds => json!({"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}),
        Kind::Changes => json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}),
        Kind::Presses => json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}),
    };
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
        "annotations": annotations,
    })
}

/// Checks the arguments against the tool's own description of them, so a mistyped name gets an
/// answer saying so, rather than being quietly ignored. The bounds the description gives are
/// the bounds that are enforced.
fn check_arguments(tool: &Value, args: &Map<String, Value>) -> Result<(), String> {
    let schema = &tool["inputSchema"];
    let properties = schema["properties"].as_object().ok_or("this tool has no description of its arguments")?;
    for (name, value) in args {
        let Some(property) = properties.get(name) else {
            let known: Vec<&str> = properties.keys().map(String::as_str).collect();
            return Err(format!("there's no argument called {name}. This tool takes: {}", known.join(", ")));
        };
        check_argument(name, property, value)?;
    }
    for name in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !args.contains_key(name) {
            return Err(format!("{name} is required"));
        }
    }
    Ok(())
}

/// Checks one value against its description, including what's inside a list.
fn check_argument(name: &str, property: &Value, value: &Value) -> Result<(), String> {
    let matches = match property["type"].as_str() {
        Some("string") => value.is_string(),
        Some("integer") => value.is_u64() || value.is_i64(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("array") => value.is_array(),
        Some("object") => value.is_object(),
        _ => true,
    };
    if !matches {
        return Err(format!("{name} should be of type {}", property["type"].as_str().unwrap_or("other")));
    }
    // Every whole number here is read as this size, and one too big must not count as left out.
    if property["type"] == "integer" && value.as_u64().is_some_and(|number| number > u64::from(u32::MAX)) {
        return Err(format!("{name} should be at most {}", u32::MAX));
    }
    if let Some(minimum) = property["minimum"].as_f64()
        && let Some(number) = value.as_f64()
        && number < minimum
    {
        return Err(format!("{name} should be at least {minimum}"));
    }
    if let Some(maximum) = property["maximum"].as_f64()
        && let Some(number) = value.as_f64()
        && number > maximum
    {
        return Err(format!("{name} should be at most {maximum}"));
    }
    if let Some(options) = property["enum"].as_array()
        && !options.contains(value)
    {
        let list: Vec<String> = options.iter().filter_map(Value::as_str).map(str::to_string).collect();
        return Err(format!("{name} should be one of: {}", list.join(", ")));
    }
    if let Some(items) = value.as_array() {
        if property["minItems"].as_u64().is_some_and(|least| (items.len() as u64) < least) {
            return Err(format!("{name} should have at least {} items", property["minItems"]));
        }
        if property["maxItems"].as_u64().is_some_and(|most| items.len() as u64 > most) {
            return Err(format!("{name} should have at most {} items", property["maxItems"]));
        }
        if property.get("items").is_some() {
            for item in items {
                check_argument(name, &property["items"], item)?;
            }
        }
    }
    Ok(())
}

fn tool_list(read_only_mode: bool) -> Vec<Value> {
    let location = json!({
        "profile": {"type": "string", "description": "Profile id. Default: the active profile."},
        "page": {"type": "integer", "minimum": 0, "description": "Page number from 0. Default: 0."},
        "folder": {
            "type": "array",
            "description": "To reach a key inside a folder: the [row, col] of each folder to open, outermost first. The slot at row 0, col 0 of a folder is its back key.",
            "items": {"type": "array", "items": {"type": "integer", "minimum": 0}, "minItems": 2, "maxItems": 2}
        },
        "row": {"type": "integer", "minimum": 0},
        "col": {"type": "integer", "minimum": 0},
    });
    let mut key_props = location.clone();
    if let Some(map) = key_props.as_object_mut() {
        map.insert("title".into(), json!({"type": "string"}));
        map.insert("icon".into(), json!({"type": "string", "description": "One character, such as an emoji. builtin:lantern-out is the drawn sleep icon."}));
        map.insert("color".into(), json!({"type": "string", "description": "Background, like #2F6FDB."}));
        map.insert("text_color".into(), json!({"type": "string", "description": "Title colour, like #FFFFFF."}));
        map.insert("toggle".into(), json!({"type": "boolean", "description": "Flip between two looks when pressed."}));
        map.insert("is_folder".into(), json!({"type": "boolean", "description": "Make this key a folder: a separate screen of keys. A folder has no steps, and setting it again keeps what's inside."}));
        map.insert("background".into(), json!({"type": "string", "description": "File name of a picture in the images folder, from list_images or add_image. It fills the key."}));
        map.insert("alt_title".into(), json!({"type": "string", "description": "The title for the second look of a toggle."}));
        map.insert("alt_color".into(), json!({"type": "string", "description": "The colour for the second look of a toggle."}));
        map.insert("steps".into(), json!({
            "type": "array",
            "description": "What the key does, in order. Needed unless is_folder is set. See list_actions for action ids and settings.",
            "items": {"type": "object", "properties": {
                "action": {"type": "string", "description": "plugin_id/action_id"},
                "settings": {"type": "object"},
                "delay_ms": {"type": "integer", "minimum": 0, "description": "Wait this long before this step."}
            }, "required": ["action"]}
        }));
    }
    let mut move_props = location.clone();
    if let Some(map) = move_props.as_object_mut() {
        map.insert("to_row".into(), json!({"type": "integer", "minimum": 0}));
        map.insert("to_col".into(), json!({"type": "integer", "minimum": 0}));
    }
    let mut tools = vec![
        tool("get_overview", "Overview", "Profiles, the active profile, plugins, the vault's state, what you're allowed to do, and anything waiting for the person.", Kind::Reads, json!({}), &[]),
        tool("list_actions", "List actions", "Every action a key can use, with its settings form. Each plugin's id is what request_site_access takes.", Kind::Reads, json!({}), &[]),
        tool("get_profile", "Read a profile", "All pages and keys in a profile, including the keys inside folders.", Kind::Reads, json!({"profile": {"type": "string", "description": "Profile id. Default: the active profile."}}), &[]),
        tool("get_settings", "Read settings", "The app's look, deck and background settings, and what the person has allowed you. Change the first three with update_settings.", Kind::Reads, json!({}), &[]),
        tool("list_secrets", "List secrets", "Names and allowed sites of vault secrets. Never values.", Kind::Reads, json!({}), &[]),
        tool("list_images", "List pictures", "The pictures a key's background can use.", Kind::Reads, json!({}), &[]),
        tool("get_guide", "Read a guide", "The app's own documentation. Topics: plugins (how to write one, the full reference), actions (every built-in action and its settings), config (the files), assistant (these tools), security (how credentials are kept).", Kind::Reads, json!({
            "topic": {"type": "string", "enum": GUIDES.map(|(topic, _)| topic)}
        }), &["topic"]),
        tool("check_config", "Check the setup", "Find every key that wouldn't run, and why, including keys inside folders.", Kind::Reads, json!({}), &[]),
    ];
    if read_only_mode {
        return tools;
    }
    tools.extend([
        tool("set_key", "Create or replace a key", "Create a key, or replace everything about the one in that slot: anything you leave out is cleared, so read it first with get_profile to change one thing. Give it steps, or set is_folder to make a folder. Several steps run in order, each waiting its delay_ms first. Won't replace a folder that has keys in it, unless is_folder is set.", Kind::Changes, key_props, &["row", "col"]),
        tool("move_key", "Move a key", "Move a key to an empty slot on the same screen. It keeps everything, including its approval and what's inside it if it's a folder.", Kind::Changes, move_props, &["row", "col", "to_row", "to_col"]),
        tool("clear_key", "Remove a key", "Remove a key. Removing a folder removes the keys inside it.", Kind::Changes, location.clone(), &["row", "col"]),
        tool("create_profile", "Add a profile", "Add a profile with one empty page, and say its id. Doesn't switch to it.", Kind::Adds, json!({
            "name": {"type": "string"},
            "rows": {"type": "integer", "minimum": 1, "maximum": 8, "description": "Default: 3."},
            "cols": {"type": "integer", "minimum": 1, "maximum": 16, "description": "Default: 5."}
        }), &["name"]),
        tool("set_active_profile", "Switch profile", "Make a profile the one the deck and the app show.", Kind::Changes, json!({
            "profile": {"type": "string", "description": "Profile id."}
        }), &["profile"]),
        tool("add_page", "Add a page", "Add an empty page to a profile.", Kind::Adds, json!({
            "profile": {"type": "string", "description": "Profile id. Default: the active profile."},
            "name": {"type": "string"}
        }), &["name"]),
        tool("rename_page", "Rename a page", "Rename a page. Keys that go to it by name need updating too.", Kind::Changes, json!({
            "profile": {"type": "string", "description": "Profile id. Default: the active profile."},
            "page": {"type": "integer", "minimum": 0, "description": "Page number from 0."},
            "name": {"type": "string"}
        }), &["page", "name"]),
        tool("add_preset", "Add a preset", "Add a ready-made action to the app's action list, or replace the preset with that name.", Kind::Changes, json!({
            "name": {"type": "string"},
            "icon": {"type": "string"},
            "color": {"type": "string", "description": "Background, like #2F6FDB."},
            "action": {"type": "string", "description": "plugin_id/action_id"},
            "settings": {"type": "object"}
        }), &["name", "action"]),
        tool("remove_preset", "Remove a preset", "Take a preset out of the action list. Keys made from it stay.", Kind::Changes, json!({"name": {"type": "string"}}), &["name"]),
        tool("add_image", "Add a picture", "Copy a PNG file from this computer into the images folder, and say the name to use as a key's background. A square picture of at least 144 by 144 looks best.", Kind::Adds, json!({
            "path": {"type": "string", "description": "Full path of a PNG file, up to 8 MB."}
        }), &["path"]),
        tool("update_settings", "Change settings", "Change how the app looks, the deck's brightness, and whether it keeps running in the background. Give only what should change. Security and assistant settings stay with the person.", Kind::Changes, json!({
            "theme": {"type": "string", "enum": THEMES},
            "accent": {"type": "string", "description": "The highlight colour, like #E8743B."},
            "scale": {"type": "number", "minimum": 0.75, "maximum": 1.5, "description": "How large the app's window contents are."},
            "tray_icon": {"type": "string", "enum": TRAY_ICONS, "description": "The menu bar or tray icon."},
            "brightness": {"type": "integer", "minimum": 0, "maximum": 100},
            "deck_enabled": {"type": "boolean", "description": "Whether the app uses a connected USB deck."},
            "run_in_background": {"type": "boolean", "description": "Whether closing the window leaves the deck working."}
        }), &[]),
        tool("request_secret", "Ask for a secret", "Ask the person to enter a password or key in the app. You get a reference to use, never the value. Check list_secrets first: there's no need to ask for one the vault already has.", Kind::Adds, json!({
            "name": {"type": "string", "description": "Letters, numbers, dots, dashes, underscores."},
            "hosts": {"type": "array", "items": {"type": "string"}, "description": "The sites it may be sent to."},
            "reason": {"type": "string", "description": "Shown to the person."}
        }), &["name", "hosts", "reason"]),
        tool("request_site_access", "Ask to allow a site", "Ask the person to let a plugin contact a site. set_key does this by itself for a key's address.", Kind::Adds, json!({
            "plugin": {"type": "string", "description": "The plugin's id, from list_actions."},
            "host": {"type": "string"},
            "reason": {"type": "string", "description": "Shown to the person."}
        }), &["plugin", "host", "reason"]),
        tool("press_key", "Press a key", "Run a key, to try it. Keys that act on the computer must be approved by the person first. Page changes, sleep and the screen saver only work from the deck or the app.", Kind::Presses, location, &["row", "col"]),
    ]);
    tools
}

fn str_arg<'a>(args: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

fn required_str<'a>(args: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    str_arg(args, name).filter(|s| !s.is_empty()).ok_or_else(|| format!("{name} is required"))
}

/// What an assistant sees in place of a credential that's stored in plain text.
const HIDDEN_CREDENTIAL: &str = "[hidden: a credential in plain text. The person needs to move it to the vault]";

fn u32_arg(args: &Map<String, Value>, name: &str) -> Option<u32> {
    args.get(name).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())
}

fn required_u32(args: &Map<String, Value>, name: &str) -> Result<u32, String> {
    u32_arg(args, name).ok_or_else(|| format!("{name} is required"))
}

/// A colour setting: empty, or the form the app can read.
fn color_arg(args: &Map<String, Value>, name: &str) -> Result<String, String> {
    let text = str_arg(args, name).unwrap_or("");
    let valid = text.strip_prefix('#').is_some_and(|hex| hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()));
    if !text.is_empty() && !valid {
        return Err(format!("{name} should look like #2F6FDB"));
    }
    Ok(text.to_string())
}

impl McpServer {
    pub fn new(ws: Workspace, engine: Engine) -> Self {
        Self { ws, engine: Arc::new(engine) }
    }

    /// Serves requests until the client closes the connection.
    pub fn serve(mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        let mut reader = input;
        loop {
            let mut line = Vec::new();
            // One limit per message, so a runaway client can't exhaust memory.
            match read_line_capped(&mut reader, MAX_LINE_BYTES, &mut line)? {
                LineOutcome::End => return Ok(()),
                LineOutcome::TooLong => {
                    let reply = error_reply(&Value::Null, -32600, "message too large");
                    writeln!(output, "{reply}")?;
                    output.flush()?;
                    // Throw away the rest of the oversized line.
                    discard_line(&mut reader, MAX_LINE_BYTES)?;
                    continue;
                }
                LineOutcome::Whole => {}
            }
            // A client may send anything, including bytes that aren't text. That is a bad
            // message, not a reason to stop serving.
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(&text) {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
    }

    /// Handles one message. Notifications get no answer.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => return Some(error_reply(&Value::Null, -32700, "not valid JSON")),
        };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let Some(method) = method else {
            // Not a notification either: it has no method to answer.
            return Some(error_reply(&id.unwrap_or(Value::Null), -32600, "invalid request: no method"));
        };
        let Some(id) = id else {
            // A notification: no answer.
            return None;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        Some(match self.dispatch(method, &params) {
            Ok(mut result) => {
                if let Some(map) = result.as_object_mut() {
                    map.insert("resultType".into(), json!("complete"));
                    let meta = map.entry("_meta").or_insert_with(|| json!({}));
                    meta[META_SERVER] = server_info();
                }
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            Err((code, text, data)) => {
                let mut reply = error_reply(&id, code, &text);
                if let Some(data) = data {
                    reply["error"]["data"] = data;
                }
                reply
            }
        })
    }

    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String, Option<Value>)> {
        if let Some(requested) = params.pointer(&format!("/_meta/{}", META_VERSION.replace('/', "~1"))).and_then(Value::as_str)
            && !MODERN_VERSIONS.contains(&requested)
            && !LEGACY_VERSIONS.contains(&requested)
        {
            return Err((
                -32022,
                "Unsupported protocol version".into(),
                Some(json!({"supported": MODERN_VERSIONS, "requested": requested})),
            ));
        }
        match method {
            "server/discover" => Ok(json!({
                "supportedVersions": MODERN_VERSIONS,
                "capabilities": {"tools": {}},
                "instructions": INSTRUCTIONS,
                "ttlMs": LIST_TTL_MS,
                "cacheScope": "private",
            })),
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if LEGACY_VERSIONS.contains(&asked) { asked } else { LEGACY_VERSIONS[0] };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": server_info(),
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => {
                self.ws.check_reload();
                Ok(json!({
                    "tools": tool_list(self.ws.settings.mcp.read_only),
                    "ttlMs": LIST_TTL_MS,
                    "cacheScope": "private",
                }))
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let empty = Map::new();
                let args = params.get("arguments").and_then(Value::as_object).unwrap_or(&empty);
                // One list of tools for this call, used for every check.
                let tools = tool_list(false);
                if !tools.iter().any(|tool| tool["name"] == name) {
                    return Err((-32602, format!("unknown tool: {name}"), None));
                }
                Ok(match self.call_tool(&tools, name, args) {
                    Ok(value) => {
                        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                        json!({"content": [{"type": "text", "text": text}], "structuredContent": value, "isError": false})
                    }
                    Err(text) => json!({"content": [{"type": "text", "text": text}], "isError": true}),
                })
            }
            other => Err((-32601, format!("unknown method: {other}"), None)),
        }
    }

    fn call_tool(&mut self, tools: &[Value], name: &str, args: &Map<String, Value>) -> Result<Value, String> {
        self.ws.check_reload();
        self.engine.refresh_plugins_if_changed();
        if !self.ws.settings.mcp.enabled {
            return Err("the person has turned off assistant access in the app's settings".into());
        }
        // A tool that only reads is marked as such in its own description, so there is one
        // list of tools and not two that could disagree.
        let read_only_tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .is_some_and(|tool| tool["annotations"]["readOnlyHint"] == Value::Bool(true));
        if self.ws.settings.mcp.read_only && !read_only_tool {
            return Err("assistant access is read-only right now".into());
        }
        let tool = tools.iter().find(|tool| tool["name"] == name).ok_or_else(|| format!("unknown tool: {name}"))?;
        check_arguments(tool, args)?;
        if !read_only_tool {
            audit(&self.ws.paths, "mcp", name);
        }
        // The vault locks itself after a quiet spell, as it does in the app.
        let minutes = u64::from(self.ws.settings.security.auto_lock_minutes);
        if let Ok(mut vault) = self.engine.vault.lock() {
            let _ = vault.reload();
            vault.lock_if_idle(Duration::from_secs(minutes * 60));
        }
        match name {
            "get_overview" => Ok(self.overview()),
            "list_actions" => Ok(self.list_actions()),
            "get_profile" => {
                let id = self.profile_id(args)?;
                let mut profile = self.ws.profiles[&id].clone();
                self.hide_credentials(&mut profile);
                serde_json::to_value(&profile).map_err(|e| e.to_string())
            }
            "get_settings" => Ok(self.settings_view()),
            "list_secrets" => self.list_secrets(),
            "list_images" => self.list_images(),
            "get_guide" => Self::guide(args),
            "check_config" => Ok(self.check_config()),
            "set_key" => self.set_key(args),
            "move_key" => self.move_key(args),
            "clear_key" => self.clear_key(args),
            "create_profile" => self.create_profile(args),
            "set_active_profile" => self.set_active_profile(args),
            "add_page" => self.add_page(args),
            "rename_page" => self.rename_page(args),
            "add_preset" => self.add_preset(args),
            "remove_preset" => self.remove_preset(args),
            "add_image" => self.add_image(args),
            "update_settings" => self.update_settings(args),
            "request_secret" => self.request_secret(args),
            "request_site_access" => self.request_site_access(args),
            "press_key" => self.press_key(args),
            other => Err(format!("unknown tool: {other}")),
        }
    }

    fn profile_id(&self, args: &Map<String, Value>) -> Result<String, String> {
        let id = str_arg(args, "profile").unwrap_or(&self.ws.settings.active_profile);
        if !self.ws.profiles.contains_key(id) {
            return Err(format!("no profile called {id}"));
        }
        Ok(id.to_string())
    }

    fn overview(&self) -> Value {
        let profiles: Vec<Value> = self
            .ws
            .profiles
            .iter()
            .map(|(id, p)| json!({"id": id, "name": p.name, "rows": p.rows, "cols": p.cols, "pages": p.pages.iter().map(|page| &page.name).collect::<Vec<_>>()}))
            .collect();
        let vault = self.engine.vault.lock().map(|vault| format!("{:?}", vault.state())).unwrap_or_default();
        json!({
            "active_profile": self.ws.settings.active_profile,
            "profiles": profiles,
            "plugins": self.engine.plugin_statuses(),
            "vault": vault,
            "waiting_for_person": Pending::load(&self.ws.paths),
            "config_problems": self.ws.errors,
            "you_may": self.allowed_view(),
        })
    }

    /// What the person has chosen to let an assistant do.
    fn allowed_view(&self) -> Value {
        json!({
            "make_changes": !self.ws.settings.mcp.read_only,
            "press_keys_that_use_secrets": self.ws.settings.security.mcp_may_use_secrets,
        })
    }

    fn settings_view(&self) -> Value {
        let settings = &self.ws.settings;
        json!({
            "theme": settings.ui.theme,
            "accent": settings.ui.accent,
            "scale": settings.ui.scale,
            "tray_icon": settings.ui.tray_icon,
            "brightness": settings.hardware.brightness,
            "deck_enabled": settings.hardware.enabled,
            "run_in_background": settings.run_in_background,
            "you_may": self.allowed_view(),
        })
    }

    fn guide(args: &Map<String, Value>) -> Result<Value, String> {
        let topic = required_str(args, "topic")?;
        let (_, guide) = GUIDES
            .iter()
            .find(|(name, _)| *name == topic)
            .ok_or_else(|| format!("no guide on {topic}. There are: {}", GUIDES.map(|(name, _)| name).join(", ")))?;
        Ok(json!({"topic": topic, "guide": guide}))
    }

    fn list_images(&self) -> Result<Value, String> {
        let mut images: Vec<String> = std::fs::read_dir(self.ws.paths.images_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".png") && !name.starts_with('.'))
            .collect();
        images.sort();
        Ok(json!({"images": images}))
    }

    fn list_actions(&self) -> Value {
        let plugins: Vec<Value> = self
            .engine
            .plugin_infos()
            .into_iter()
            .map(|info| {
                let actions: Vec<Value> = info
                    .actions
                    .iter()
                    .filter(|a| !a.hidden)
                    .map(|a| json!({
                        "action": format!("{}/{}", info.id, a.id),
                        "name": a.name,
                        "description": a.description,
                        "icon": a.icon,
                        "needs_person_approval": a.risk == pyroporch_sdk::Risk::High,
                        "can_refresh_on_a_timer": a.refreshable,
                        "fields": a.fields,
                    }))
                    .collect();
                json!({"plugin_id": info.id, "plugin": info.name, "actions": actions})
            })
            .collect();
        json!({"plugins": plugins, "secret_rule": "Secret fields and credentials only accept {{secret:name}} references."})
    }

    fn list_secrets(&self) -> Result<Value, String> {
        let mut vault = self.engine.vault.lock().map_err(|_| "the vault is unavailable")?;
        vault.reload()?;
        Ok(json!({"secrets": vault.list()}))
    }

    fn check_config(&self) -> Value {
        let mut problems = Vec::new();
        for (id, profile) in &self.ws.profiles {
            for (index, page) in profile.pages.iter().enumerate() {
                self.check_screen(id, index, page, &mut Vec::new(), &mut problems);
            }
        }
        json!({"file_problems": self.ws.errors, "key_problems": problems})
    }

    /// Checks one screen and the folders on it, saying where each key is in the form the other
    /// tools take.
    fn check_screen(&self, id: &str, index: usize, screen: &Page, path: &mut Vec<[u32; 2]>, problems: &mut Vec<Value>) {
        for key in &screen.keys {
            let blocks = self.engine.check_key(key);
            if !blocks.is_empty() {
                problems.push(json!({
                    "profile": id, "page": index, "folder": path, "row": key.row, "col": key.col, "title": key.title,
                    "problems": blocks.iter().map(Block::to_string).collect::<Vec<_>>(),
                }));
            }
            if let Some(folder) = &key.folder {
                path.push([key.row, key.col]);
                self.check_screen(id, index, folder, path, problems);
                path.pop();
            }
        }
    }

    fn folder_path(args: &Map<String, Value>) -> Vec<(u32, u32)> {
        args.get("folder")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|pair| {
                        let row = pair.get(0)?.as_u64()?;
                        let col = pair.get(1)?.as_u64()?;
                        Some((u32::try_from(row).ok()?, u32::try_from(col).ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The profile, its grid, and the screen the location points at.
    fn screen_mut(&mut self, args: &Map<String, Value>) -> Result<(String, usize, u32, u32, &mut Page), String> {
        let id = self.profile_id(args)?;
        let page = u32_arg(args, "page").unwrap_or(0) as usize;
        let path = Self::folder_path(args);
        let profile = self.ws.profiles.get_mut(&id).ok_or("no such profile")?;
        let (rows, cols) = (profile.rows, profile.cols);
        let count = profile.pages.len();
        let top = profile.pages.get_mut(page).ok_or_else(|| format!("that profile has {count} page(s)"))?;
        let screen = top.descend_mut(&path).ok_or("there is no folder there")?;
        Ok((id, page, rows, cols, screen))
    }

    fn parse_steps(args: &Map<String, Value>) -> Result<Vec<Step>, String> {
        let steps = args.get("steps").and_then(Value::as_array).ok_or("steps is required")?;
        steps
            .iter()
            .map(|step| {
                let action = step.get("action").and_then(Value::as_str).ok_or("each step needs an action")?;
                Ok(Step {
                    action: action.to_string(),
                    settings: step.get("settings").and_then(Value::as_object).cloned().unwrap_or_default(),
                    delay_ms: step.get("delay_ms").and_then(Value::as_u64).unwrap_or(0),
                })
            })
            .collect()
    }

    /// A file edited by hand can hold a credential in plain text. It's refused when a key runs,
    /// and an assistant reading the profile doesn't get to see it either.
    fn hide_credentials(&self, profile: &mut Profile) {
        for page in &mut profile.pages {
            page.for_each_key_mut(&mut |key| {
                for step in &mut key.steps {
                    let spec = self.engine.spec(&step.action);
                    for (name, value) in &mut step.settings {
                        let one = Map::from_iter([(name.clone(), value.clone())]);
                        if !plaintext_findings(spec.as_ref(), &one).is_empty() {
                            *value = Value::String(HIDDEN_CREDENTIAL.into());
                        }
                    }
                }
            });
        }
    }

    fn refreshes_with_secret(&self, step: &Step) -> bool {
        let refreshes = step.settings.get("refresh_seconds").and_then(Value::as_f64).is_some_and(|seconds| seconds >= 1.0);
        refreshes && self.engine.is_refreshable(&step.action) && !find_refs(&Value::Object(step.settings.clone()).to_string()).is_empty()
    }

    fn set_key(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let (row, col) = (required_u32(args, "row")?, required_u32(args, "col")?);
        let is_folder = args.get("is_folder").and_then(Value::as_bool).unwrap_or(false);
        if is_folder && args.contains_key("steps") {
            return Err("a folder has no steps: it opens a screen of keys".into());
        }
        if is_folder && Self::folder_path(args).len() >= MAX_FOLDER_DEPTH {
            return Err(format!("folders can only go {MAX_FOLDER_DEPTH} deep"));
        }
        let steps = if is_folder { Vec::new() } else { Self::parse_steps(args)? };
        if !Self::folder_path(args).is_empty() && (row, col) == BACK_SLOT {
            return Err("row 0, col 0 of a folder is its back key".into());
        }
        let background = str_arg(args, "background").unwrap_or("").to_string();
        if !background.is_empty() && !self.ws.paths.image(&background).is_some_and(|path| path.exists()) {
            return Err(format!("no picture called {background} in the images folder"));
        }
        // Check before anything is saved: a plain credential must never reach the file.
        let mut refusals = Vec::new();
        for step in &steps {
            for block in self.engine.check_step(step) {
                match block {
                    Block::PlainCredential(_) | Block::UnknownAction(_) | Block::MissingSetting(_) => {
                        refusals.push(block.to_string());
                    }
                    Block::NeedsApproval => {}
                }
            }
        }
        // A timer runs in the app, where secrets can be used, so it can't be a way round the
        // person's choice that an assistant's keys can't use them.
        if !self.ws.settings.security.mcp_may_use_secrets && steps.iter().any(|step| self.refreshes_with_secret(step)) {
            refusals.push("a key that refreshes on a timer can't use a vault secret unless the person allows assistants to use secrets".to_string());
        }
        if !refusals.is_empty() {
            return Err(format!("Nothing was saved. {}", refusals.join("; ")));
        }

        let text = |name: &str| str_arg(args, name).unwrap_or("").to_string();
        let mut key = Key {
            row,
            col,
            title: text("title"),
            icon: text("icon"),
            background,
            color: color_arg(args, "color")?,
            text_color: color_arg(args, "text_color")?,
            toggle: args.get("toggle").and_then(Value::as_bool).unwrap_or(false),
            alt_title: text("alt_title"),
            alt_color: color_arg(args, "alt_color")?,
            steps,
            ..Key::default()
        };
        let (id, page, rows, cols, screen) = self.screen_mut(args)?;
        if row >= rows || col >= cols {
            return Err(format!("that profile's grid is {rows} rows by {cols} columns"));
        }
        let old = screen.key_at(row, col);
        // Nothing but clear_key throws away what's inside a folder.
        if !is_folder && old.and_then(|old| old.folder.as_ref()).is_some_and(|folder| !folder.keys.is_empty()) {
            return Err("that key is a folder with keys in it, so nothing was changed. Move them out, or use clear_key if the person wants them gone.".into());
        }
        // An unchanged key keeps its approval; anything else starts unapproved.
        if let Some(old) = old
            && old.steps == key.steps
        {
            key.approved = old.approved.clone();
        }
        // A folder that's replaced by a folder keeps what's in it.
        if is_folder {
            key.folder = Some(old.and_then(|old| old.folder.clone()).unwrap_or_else(|| {
                Box::new(Page {
                    name: key.title.clone(),
                    keys: Vec::new(),
                })
            }));
        }
        screen.put(key.clone());
        self.ws.save_profile(&id)?;

        let mut needs_person = Vec::new();
        if self.engine.check_key(&key).contains(&Block::NeedsApproval) {
            needs_person.push("This key acts on the computer. The person must approve it in the app before it can run.".to_string());
        }
        let grants = Grants::load(&self.ws.paths);
        for (plugin, site) in self.engine.sites_called(&key) {
            let allowed = grants.allows_host(&plugin, &site);
            if !allowed {
                Pending::request_host(&self.ws.paths, &plugin, &site, "An assistant set up a key that calls it.");
                needs_person.push(format!("The person must allow {site} in the app. A request is waiting for them."));
            }
        }
        Ok(json!({"saved": true, "profile": id, "page": page, "row": row, "col": col, "needs_person": needs_person}))
    }

    fn move_key(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let from = (required_u32(args, "row")?, required_u32(args, "col")?);
        let to = (required_u32(args, "to_row")?, required_u32(args, "to_col")?);
        if !Self::folder_path(args).is_empty() && (from == BACK_SLOT || to == BACK_SLOT) {
            return Err("row 0, col 0 of a folder is its back key".into());
        }
        let (id, _, rows, cols, screen) = self.screen_mut(args)?;
        if to.0 >= rows || to.1 >= cols {
            return Err(format!("that profile's grid is {rows} rows by {cols} columns"));
        }
        if screen.key_at(to.0, to.1).is_some() {
            return Err("there's already a key in that slot. Move or clear it first".into());
        }
        let mut key = screen.remove_at(from.0, from.1).ok_or("there is no key there")?;
        key.row = to.0;
        key.col = to.1;
        screen.put(key);
        self.ws.save_profile(&id)?;
        Ok(json!({"moved": true, "row": to.0, "col": to.1}))
    }

    fn clear_key(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let (row, col) = (required_u32(args, "row")?, required_u32(args, "col")?);
        let (id, _, _, _, screen) = self.screen_mut(args)?;
        let removed = screen.remove_at(row, col).is_some();
        self.ws.save_profile(&id)?;
        Ok(json!({"removed": removed}))
    }

    fn create_profile(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let profile = Profile {
            name: required_str(args, "name")?.to_string(),
            rows: u32_arg(args, "rows").unwrap_or(3),
            cols: u32_arg(args, "cols").unwrap_or(5),
            ..Profile::default()
        };
        profile.validate()?;
        Ok(json!({"id": self.ws.add_profile(profile)?}))
    }

    fn set_active_profile(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        required_str(args, "profile")?;
        let id = self.profile_id(args)?;
        self.ws.settings.active_profile = id.clone();
        self.ws.save_settings()?;
        Ok(json!({"active_profile": id}))
    }

    fn add_page(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = self.profile_id(args)?;
        let name = required_str(args, "name")?.to_string();
        let profile = self.ws.profiles.get_mut(&id).ok_or("no such profile")?;
        profile.pages.push(Page {
            name,
            keys: Vec::new(),
        });
        let page = profile.pages.len() - 1;
        self.ws.save_profile(&id)?;
        Ok(json!({"profile": id, "page": page}))
    }

    fn rename_page(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = self.profile_id(args)?;
        let index = required_u32(args, "page")? as usize;
        let name = required_str(args, "name")?.to_string();
        let profile = self.ws.profiles.get_mut(&id).ok_or("no such profile")?;
        let count = profile.pages.len();
        let page = profile.pages.get_mut(index).ok_or_else(|| format!("that profile has {count} page(s)"))?;
        page.name = name.clone();
        self.ws.save_profile(&id)?;
        Ok(json!({"profile": id, "page": index, "name": name}))
    }

    fn add_preset(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let name = required_str(args, "name")?.to_string();
        let action = required_str(args, "action")?.to_string();
        let settings = args.get("settings").and_then(Value::as_object).cloned().unwrap_or_default();
        let step = Step {
            action: action.clone(),
            settings: settings.clone(),
            delay_ms: 0,
        };
        for block in self.engine.check_step(&step) {
            if matches!(block, Block::PlainCredential(_) | Block::UnknownAction(_)) {
                return Err(format!("Nothing was saved. {block}"));
            }
        }
        // Checked before the old one goes, so a bad colour doesn't lose it.
        let color = color_arg(args, "color")?;
        self.ws.settings.presets.retain(|p| p.name != name);
        self.ws.settings.presets.push(Preset {
            name: name.clone(),
            icon: str_arg(args, "icon").unwrap_or("").into(),
            color,
            action,
            settings,
        });
        self.ws.save_settings()?;
        Ok(json!({"saved": name}))
    }

    fn remove_preset(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let name = required_str(args, "name")?;
        let before = self.ws.settings.presets.len();
        self.ws.settings.presets.retain(|p| p.name != name);
        let removed = self.ws.settings.presets.len() != before;
        if removed {
            self.ws.save_settings()?;
        }
        Ok(json!({"removed": removed}))
    }

    fn add_image(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let path = std::path::Path::new(required_str(args, "path")?);
        let bytes = read_capped(path, MAX_IMAGE_BYTES)?;
        Ok(json!({"background": store_image(&self.ws.paths, &bytes)?}))
    }

    fn update_settings(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        if args.is_empty() {
            return Err("say which settings should change".into());
        }
        // Everything is checked before anything changes.
        let theme = str_arg(args, "theme");
        if let Some(theme) = theme.filter(|theme| !THEMES.contains(theme)) {
            return Err(format!("no theme called {theme}. There are: {}", THEMES.join(", ")));
        }
        let tray_icon = str_arg(args, "tray_icon");
        if let Some(icon) = tray_icon.filter(|icon| !TRAY_ICONS.contains(icon)) {
            return Err(format!("no tray icon called {icon}. There are: {}", TRAY_ICONS.join(", ")));
        }
        let accent = color_arg(args, "accent")?;
        if args.contains_key("accent") && accent.is_empty() {
            return Err("accent should look like #E8743B".into());
        }
        let scale = args.get("scale").and_then(Value::as_f64);
        if scale.is_some_and(|scale| !SCALE_RANGE.contains(&scale)) {
            return Err(format!("scale should be from {} to {}", SCALE_RANGE.start(), SCALE_RANGE.end()));
        }
        let brightness = u32_arg(args, "brightness");
        if args.contains_key("brightness") && !brightness.is_some_and(|brightness| brightness <= 100) {
            return Err("brightness should be from 0 to 100".into());
        }

        let settings = &mut self.ws.settings;
        if let Some(theme) = theme {
            settings.ui.theme = theme.to_string();
        }
        if let Some(icon) = tray_icon {
            settings.ui.tray_icon = icon.to_string();
        }
        if !accent.is_empty() {
            settings.ui.accent = accent;
        }
        if let Some(scale) = scale {
            settings.ui.scale = scale as f32;
        }
        if let Some(brightness) = brightness {
            settings.hardware.brightness = brightness as u8;
        }
        if let Some(enabled) = args.get("deck_enabled").and_then(Value::as_bool) {
            settings.hardware.enabled = enabled;
        }
        if let Some(background) = args.get("run_in_background").and_then(Value::as_bool) {
            settings.run_in_background = background;
        }
        self.ws.save_settings()?;
        Ok(self.settings_view())
    }

    fn request_secret(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let name = required_str(args, "name")?;
        if !valid_name(name) {
            return Err("a secret's name can only use letters, numbers, dots, dashes and underscores".into());
        }
        let hosts: Vec<String> = args
            .get("hosts")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let hosts = normalize_hosts(&hosts)?;
        if hosts.is_empty() {
            return Err("say which sites it may be sent to".into());
        }
        // The vault already has it for these sites, so there's nothing to bother the person with.
        let stored = {
            let mut vault = self.engine.vault.lock().map_err(|_| "the vault is unavailable")?;
            vault.reload()?;
            vault.list().into_iter().find(|secret| secret.name == name)
        };
        if stored.is_some_and(|secret| hosts.iter().all(|host| secret.allowed_hosts.contains(host))) {
            return Ok(json!({
                "reference": pyroporch_sdk::secret_ref(name),
                "status": "The vault already has this for those sites. Use the reference in settings; you will never see the value.",
            }));
        }
        Pending::request_secret(&self.ws.paths, name, &hosts, required_str(args, "reason")?);
        Ok(json!({
            "reference": pyroporch_sdk::secret_ref(name),
            "status": "The person has been asked to enter it in the app. Use the reference in settings; you will never see the value.",
        }))
    }

    fn request_site_access(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let plugin = required_str(args, "plugin")?;
        if !self.engine.plugin_statuses().iter().any(|status| status.id == plugin) {
            return Err(format!("there's no plugin with the id {plugin}. list_actions gives the ids"));
        }
        let host = normalize_hosts(&[required_str(args, "host")?.to_string()])?;
        let host = host.first().ok_or("host is required")?;
        Pending::request_host(&self.ws.paths, plugin, host, required_str(args, "reason")?);
        Ok(json!({"status": "The person has been asked in the app."}))
    }

    fn press_key(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let (row, col) = (required_u32(args, "row")?, required_u32(args, "col")?);
        let id = self.profile_id(args)?;
        let page = u32_arg(args, "page").unwrap_or(0) as usize;
        let path = Self::folder_path(args);
        let key = self
            .ws
            .profiles
            .get(&id)
            .and_then(|p| p.pages.get(page))
            .and_then(|p| p.descend(&path))
            .and_then(|p| p.key_at(row, col))
            .cloned()
            .ok_or("there is no key there")?;
        if key.folder.is_some() {
            return Err("that key is a folder. Use the folder argument to reach the keys inside it".into());
        }
        let secrets_allowed = self.ws.settings.security.mcp_may_use_secrets && self.unlock_from_device();
        // The key runs on its own thread, so one that waits a long time doesn't stop the
        // server answering anyone else.
        let (sender, receiver) = std::sync::mpsc::channel();
        let engine = self.engine.clone();
        std::thread::spawn(move || {
            let _ = sender.send(engine.run_key(&key, Trigger::Press, "mcp", secrets_allowed));
        });
        let report = receiver.recv_timeout(PRESS_TIMEOUT).map_err(|_| {
            "that key is taking too long, so it wasn't waited for. Press it in the app instead.".to_string()
        })?;
        let mut message = report.message;
        // This process doesn't hold the deck or the window. A saved brightness reaches the app
        // through the config, but page changes and sleep have to come from the app itself.
        if !report.navigate.is_empty() {
            message = "That only works from the deck or the app.".into();
        }
        // The key may have taken a while, and the person may have changed their settings in that
        // time. Saving what was read before it ran would undo that, access switches included.
        self.ws.check_reload();
        let may_save = self.ws.settings.mcp.enabled && !self.ws.settings.mcp.read_only;
        for control in report.deck {
            if matches!(control, DeckControl::Sleep | DeckControl::ScreenSaver(_)) {
                message = "That only works from the deck or the app.".into();
                continue;
            }
            if !may_save {
                message = "Assistant access changed while that key ran, so nothing was saved.".into();
                continue;
            }
            let current = self.ws.settings.hardware.brightness;
            if let Some(brightness) = control.brightness_after(current) {
                self.ws.settings.hardware.brightness = brightness;
                self.ws.save_settings()?;
                message = format!("brightness {brightness}%");
            }
        }
        Ok(json!({"ok": report.ok, "message": message, "title": report.title, "icon": report.icon, "color": report.color}))
    }

    #[cfg(feature = "os-keychain")]
    fn unlock_from_device(&mut self) -> bool {
        self.engine.vault.lock().is_ok_and(|mut vault| vault.unlock_from_device().is_ok())
    }

    #[cfg(not(feature = "os-keychain"))]
    fn unlock_from_device(&mut self) -> bool {
        false
    }
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// What one read of the client's line brought.
enum LineOutcome {
    /// A whole line, ending in a newline or at the end of the input.
    Whole,
    /// Longer than the limit; the rest of it is still waiting to be thrown away.
    TooLong,
    /// Nothing more to read.
    End,
}

/// Reads one line, at most `limit` bytes, into `out`.
fn read_line_capped(reader: &mut impl BufRead, limit: usize, out: &mut Vec<u8>) -> std::io::Result<LineOutcome> {
    let mut complete = false;
    let mut at_end = false;
    while out.len() < limit {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            // The end of the input. What we have is the last line.
            at_end = true;
            break;
        }
        match available.iter().position(|b| *b == b'\n') {
            Some(at) => {
                if out.len() + at < limit {
                    out.extend_from_slice(&available[..=at]);
                    reader.consume(at + 1);
                    complete = true;
                } else {
                    // The line is longer than the limit, so only what fits is kept.
                    let room = limit - out.len();
                    out.extend_from_slice(&available[..room]);
                    reader.consume(room);
                }
                break;
            }
            None => {
                let room = limit - out.len();
                let take = room.min(available.len());
                let short = take < available.len();
                out.extend_from_slice(&available[..take]);
                reader.consume(take);
                if short {
                    // The limit is reached. The rest of the line is thrown away.
                    break;
                }
            }
        }
    }
    Ok(if complete {
        LineOutcome::Whole
    } else if at_end {
        if out.is_empty() { LineOutcome::End } else { LineOutcome::Whole }
    } else {
        LineOutcome::TooLong
    })
}

/// Reads a file, but never more than `limit` bytes, and never from something that isn't a plain
/// file: a device or a pipe reports no size at all, and reading it would never end.
fn read_capped(path: &std::path::Path, limit: usize) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("couldn't read that file: {e}"))?;
    if !meta.is_file() {
        return Err("that isn't a plain file".into());
    }
    if meta.len() > limit as u64 {
        return Err("that picture is too big".into());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("couldn't read that file: {e}"))?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("couldn't read that file: {e}"))?;
    if bytes.len() > limit {
        return Err("that picture is too big".into());
    }
    Ok(bytes)
}

/// Throws away the rest of a line, up to a limit, so a runaway client can't exhaust memory.
fn discard_line(reader: &mut impl BufRead, limit: usize) -> std::io::Result<()> {    let mut thrown = 0;
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(());
        }
        match available.iter().position(|b| *b == b'\n') {
            Some(at) => {
                reader.consume(at + 1);
                return Ok(());
            }
            None => {
                let len = available.len();
                thrown += len;
                reader.consume(len);
                if thrown >= limit {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::vault::{KdfParams, Vault};
    use pyroporch_sdk::{ActionSpec, Field, FieldKind, HostHandle, Outcome, Plugin, PluginInfo, Risk, RunRequest};
    use std::sync::Arc;

    struct Fake;
    impl Plugin for Fake {
        fn info(&self) -> PluginInfo {
            let action = |id: &str, risk, fields| ActionSpec {
                id: String::from(id),
                name: String::from(id),
                description: String::new(),
                icon: String::new(),
                risk,
                refreshable: false,
                hidden: false,
                previews: false,
                fields,
            };
            PluginInfo {
                id: "test.fake".into(),
                name: "Fake".into(),
                version: "1".into(),
                description: String::new(),
                actions: vec![
                    action("call", Risk::Low, vec![
                        Field::new("url", "Address", FieldKind::RequestUrl),
                        Field::new("secret", "Secret", FieldKind::Secret),
                    ]),
                    action("risky", Risk::High, vec![Field::new("command", "Command", FieldKind::Text)]),
                    action("saver", Risk::Low, vec![]),
                ],
            }
        }
        fn run(&self, request: &RunRequest, _: &HostHandle) -> Result<Outcome, String> {
            let deck = (request.action == "saver").then_some(DeckControl::ScreenSaver(pyroporch_sdk::SaverTiming {
                start_now: true,
                wait_minutes: 1,
            }));
            Ok(Outcome {
                message: format!("ran {}", request.action),
                deck,
                ..Outcome::default()
            })
        }
    }

    fn server() -> (tempfile::TempDir, McpServer) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        let ws = Workspace::load(paths.clone()).unwrap();
        let vault = Vault::with_kdf(paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
        let engine = Engine::new(paths, vault, vec![Arc::new(Fake)]);
        (dir, McpServer::new(ws, engine))
    }

    fn call(server: &mut McpServer, name: &str, arguments: Value) -> (bool, String) {
        let message = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}});
        let reply = server.handle_line(&message.to_string()).unwrap();
        let result = &reply["result"];
        (result["isError"].as_bool().unwrap(), result["content"][0]["text"].as_str().unwrap().to_string())
    }

    #[test]
    fn a_slow_key_cant_undo_access_turned_off_while_it_ran() {
        let (dir, mut server) = server();
        let (failed, text) = call(&mut server, "set_key", json!({
            "row": 2, "col": 0, "title": "Slow",
            "steps": [{"action": "core.deck/brighter", "settings": {"step": 10}, "delay_ms": 400}],
        }));
        assert!(!failed, "{text}");
        let path = dir.path().join("config.toml");
        let writer = std::thread::spawn({
            let path = path.clone();
            move || {
                std::thread::sleep(Duration::from_millis(100));
                std::fs::write(&path, "[mcp]\nenabled = false\nread_only = true\n").unwrap();
            }
        });
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 0}));
        writer.join().unwrap();
        assert!(text.contains("nothing was saved"), "{text}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[mcp]\nenabled = false\nread_only = true\n");
    }

    #[test]
    fn a_plain_credential_in_a_profile_stays_hidden_from_an_assistant() {
        let (_dir, mut server) = server();
        let active = server.ws.settings.active_profile.clone();
        let mut settings = Map::new();
        settings.insert("password".into(), json!("hunter2"));
        settings.insert("note".into(), json!("kept"));
        server.ws.profiles.get_mut(&active).unwrap().pages[0].keys[0].steps = vec![Step { action: "test.fake/call".into(), settings, delay_ms: 0 }];
        server.ws.save_profile(&active).unwrap();
        let (failed, text) = call(&mut server, "get_profile", json!({}));
        assert!(!failed, "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("[hidden") && text.contains("kept"), "{text}");
    }

    #[test]
    fn a_bad_colour_doesnt_lose_the_preset_being_replaced() {
        let (_dir, mut server) = server();
        let preset = json!({"name": "P", "action": "test.fake/call", "settings": {}, "color": "#112233"});
        assert!(!call(&mut server, "add_preset", preset).0);
        let bad = json!({"name": "P", "action": "test.fake/call", "settings": {}, "color": "not a colour"});
        assert!(call(&mut server, "add_preset", bad).0);
        assert_eq!(server.ws.settings.presets[0].color, "#112233");
    }

    #[test]
    fn a_bad_folder_or_page_is_refused_rather_than_acting_on_another_screen() {
        let (_dir, mut server) = server();
        let before = call(&mut server, "get_profile", json!({})).1;
        for bad in [json!([["a", "b"]]), json!([1, 4]), json!([["x"]]), json!([[1, 2, 3]]), json!([[5_000_000_000u64, 0]])] {
            let (failed, text) = call(&mut server, "clear_key", json!({"row": 0, "col": 0, "folder": bad}));
            assert!(failed, "{text}");
        }
        let (failed, text) = call(&mut server, "clear_key", json!({"row": 0, "col": 0, "page": 5_000_000_000u64}));
        assert!(failed, "{text}");
        assert_eq!(call(&mut server, "get_profile", json!({})).1, before, "nothing changed");
    }

    #[test]
    fn modern_and_legacy_clients_can_both_connect() {
        let (_dir, mut server) = server();
        let reply = server.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}"#).unwrap();
        assert_eq!(reply["result"]["supportedVersions"][0], "2026-07-28");
        assert_eq!(reply["result"]["resultType"], "complete");
        assert_eq!(reply["result"]["_meta"][META_SERVER]["name"], "pyroporch");

        let reply = server
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#)
            .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert!(server.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());

        let reply = server.handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#).unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "set_key"));
        assert!(reply["result"]["ttlMs"].is_number());
    }

    #[test]
    fn unsupported_versions_and_methods_get_proper_errors() {
        let (_dir, mut server) = server();
        let reply = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"1900-01-01"}}}"#)
            .unwrap();
        assert_eq!(reply["error"]["code"], -32022);
        assert_eq!(reply["error"]["data"]["requested"], "1900-01-01");
        let reply = server.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#).unwrap();
        assert_eq!(reply["error"]["code"], -32601);
        let reply = server.handle_line("not json").unwrap();
        assert_eq!(reply["error"]["code"], -32700);
    }

    #[test]
    fn an_assistant_can_add_a_key_that_lands_in_the_config() {
        let (dir, mut server) = server();
        let (failed, text) = call(&mut server, "set_key", json!({
            "row": 2, "col": 0, "title": "Weather", "icon": "⛅",
            "steps": [{"action": "test.fake/call", "settings": {"url": "https://api.weather.example/now"}}]
        }));
        assert!(!failed, "{text}");
        assert!(text.contains("must allow api.weather.example"), "{text}");
        let saved = std::fs::read_to_string(dir.path().join("profiles/default.toml")).unwrap();
        assert!(saved.contains("Weather"));
        assert_eq!(Pending::load(&server.ws.paths).hosts[0].host, "api.weather.example");
    }

    #[test]
    fn plain_credentials_are_refused_before_anything_is_saved() {
        let (dir, mut server) = server();
        for settings in [
            json!({"secret": "hunter2"}),
            json!({"url": "https://x.example/?token=abc"}),
            json!({"url": "https://me:pw@x.example/"}),
        ] {
            let (failed, text) = call(&mut server, "set_key", json!({
                "row": 2, "col": 0, "title": "MARKER",
                "steps": [{"action": "test.fake/call", "settings": settings}]
            }));
            assert!(failed, "{text}");
            assert!(text.contains("Nothing was saved"), "{text}");
        }
        let saved = std::fs::read_to_string(dir.path().join("profiles/default.toml")).unwrap();
        assert!(!saved.contains("MARKER"));
        assert!(!saved.contains("hunter2"));
    }

    #[test]
    fn an_assistant_cannot_run_or_silently_change_a_risky_key() {
        let (_dir, mut server) = server();
        let set = |server: &mut McpServer, command: &str| {
            call(server, "set_key", json!({
                "row": 2, "col": 1,
                "steps": [{"action": "test.fake/risky", "settings": {"command": command}}]
            }))
        };
        let (failed, text) = set(&mut server, "ls");
        assert!(!failed);
        assert!(text.contains("must approve"), "{text}");
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 1}));
        assert!(text.contains("\"ok\": false"), "{text}");

        // The person approves it in the app.
        let active = server.ws.settings.active_profile.clone();
        let profile = server.ws.profiles.get_mut(&active).unwrap();
        profile.pages[0].key_at_mut(2, 1).unwrap().approve();
        server.ws.save_profile(&active).unwrap();
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 1}));
        assert!(text.contains("\"ok\": true"), "{text}");

        // Sending the same thing keeps the approval; changing it loses it.
        set(&mut server, "ls");
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 1}));
        assert!(text.contains("\"ok\": true"), "{text}");
        set(&mut server, "something else");
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 1}));
        assert!(text.contains("\"ok\": false"), "{text}");
    }

    #[test]
    fn an_assistant_can_build_folders_and_keys_inside_them() {
        let (_dir, mut server) = server();
        let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 0, "title": "Tools", "is_folder": true}));
        assert!(!failed, "{text}");
        let inside = json!({"row": 0, "col": 2, "folder": [[2, 0]], "title": "Inside", "steps": [{"action": "test.fake/call", "settings": {}}]});
        let (failed, text) = call(&mut server, "set_key", inside.clone());
        assert!(!failed, "{text}");
        let (_, text) = call(&mut server, "press_key", json!({"row": 0, "col": 2, "folder": [[2, 0]]}));
        assert!(text.contains("ran call"), "{text}");

        let (failed, text) = call(&mut server, "set_key", json!({"row": 0, "col": 0, "folder": [[2, 0]], "steps": []}));
        assert!(failed, "the back slot is taken: {text}");
        let (failed, _) = call(&mut server, "press_key", json!({"row": 2, "col": 0}));
        assert!(failed, "a folder isn't pressed");

        // Renaming a folder keeps what's in it.
        call(&mut server, "set_key", json!({"row": 2, "col": 0, "title": "Renamed", "is_folder": true}));
        let (_, text) = call(&mut server, "get_profile", json!({}));
        assert!(text.contains("Renamed") && text.contains("Inside"), "{text}");

        let (failed, text) = call(&mut server, "set_key", json!({"row": 0, "col": 3, "folder": [[9, 9]], "steps": []}));
        assert!(failed, "{text}");
    }

    #[test]
    fn an_assistant_can_change_brightness_through_the_config_but_not_sleep_the_deck() {
        let (dir, mut server) = server();
        call(&mut server, "set_key", json!({
            "row": 2, "col": 0, "steps": [{"action": "core.deck/set_brightness", "settings": {"percent": 40}}]
        }));
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 0}));
        assert!(text.contains("brightness 40%"), "{text}");
        let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(saved.contains("brightness = 40"), "{saved}");

        call(&mut server, "set_key", json!({"row": 2, "col": 1, "steps": [{"action": "core.deck/sleep"}]}));
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 1}));
        assert!(text.contains("only works from"), "{text}");
    }

    #[test]
    fn an_assistant_cannot_start_the_screen_saver() {
        let (_dir, mut server) = server();
        call(&mut server, "set_key", json!({"row": 2, "col": 0, "steps": [{"action": "test.fake/saver"}]}));
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 0}));
        assert!(text.contains("only works from"), "{text}");
    }

    #[test]
    fn the_action_list_leaves_out_hidden_actions() {
        let (_dir, mut server) = server();
        let (_, text) = call(&mut server, "list_actions", json!({}));
        assert!(!text.contains("core.navigation/wait"), "{text}");
        assert!(text.contains("core.deck/sleep"), "{text}");
    }

    #[test]
    fn changing_how_long_a_step_waits_needs_approval_again() {
        let (_dir, mut server) = server();
        let args = |delay: u64| json!({
            "row": 2, "col": 2,
            "steps": [{"action": "test.fake/risky"}, {"action": "test.fake/risky", "delay_ms": delay}]
        });
        call(&mut server, "set_key", args(1));
        let active = server.ws.settings.active_profile.clone();
        let profile = server.ws.profiles.get_mut(&active).unwrap();
        profile.pages[0].key_at_mut(2, 2).unwrap().approve();
        server.ws.save_profile(&active).unwrap();
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 2}));
        assert!(text.contains("\"ok\": true"), "{text}");

        call(&mut server, "set_key", args(2));
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 2}));
        assert!(text.contains("\"ok\": false"), "{text}");
    }

    #[test]
    fn no_tool_can_read_or_store_a_secret_or_approve_anything() {
        for tool in tool_list(false) {
            let name = tool["name"].as_str().unwrap();
            for banned in ["reveal", "get_secret", "set_secret", "approve", "unlock", "grant"] {
                assert!(!name.contains(banned), "{name}");
            }
        }
        let (_dir, mut server) = server();
        let (failed, text) = call(&mut server, "request_secret", json!({
            "name": "weather_key", "hosts": ["api.weather.example"], "reason": "For the weather key"
        }));
        assert!(!failed);
        assert!(text.contains("{{secret:weather_key}}"), "{text}");
        assert_eq!(Pending::load(&server.ws.paths).secrets[0].name, "weather_key");
    }

    #[test]
    fn the_person_can_turn_access_off_or_read_only() {
        let (dir, mut server) = server();
        let config = dir.path().join("config.toml");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&config, "[mcp]\nenabled = true\nread_only = true\n").unwrap();
        let (failed, text) = call(&mut server, "set_key", json!({"row": 0, "col": 0, "steps": []}));
        assert!(failed, "{text}");
        assert!(text.contains("read-only"));
        let (failed, _) = call(&mut server, "get_overview", json!({}));
        assert!(!failed);
        let reply = server.handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#).unwrap();
        assert!(!reply["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "set_key"));

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&config, "[mcp]\nenabled = false\n").unwrap();
        let (failed, text) = call(&mut server, "get_overview", json!({}));
        assert!(failed);
        assert!(text.contains("turned off"));
    }

    fn write_png(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("picture.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nnot really a picture").unwrap();
        path
    }

    #[test]
    fn tools_say_what_they_do_to_the_setup() {
        let tools = tool_list(false);
        let hint = |name: &str, hint: &str| tools.iter().find(|t| t["name"] == name).unwrap()["annotations"][hint].clone();
        assert_eq!(hint("list_actions", "readOnlyHint"), true);
        assert_eq!(hint("add_page", "destructiveHint"), false);
        assert_eq!(hint("clear_key", "destructiveHint"), true);
        assert_eq!(hint("press_key", "openWorldHint"), true);
        assert!(tools.iter().all(|t| t["title"].is_string() && t["description"].is_string()));
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_and_a_bad_argument_is_explained() {
        let (_dir, mut server) = server();
        let reply = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#)
            .unwrap();
        assert_eq!(reply["error"]["code"], -32602);

        let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 0, "titel": "Typo", "steps": []}));
        assert!(failed);
        assert!(text.contains("no argument called titel") && text.contains("title"), "{text}");
        let (failed, text) = call(&mut server, "set_key", json!({"row": "two", "col": 0, "steps": []}));
        assert!(failed);
        assert!(text.contains("row should be of type integer"), "{text}");
        let (failed, text) = call(&mut server, "set_key", json!({"col": 0, "steps": []}));
        assert!(failed);
        assert!(text.contains("row is required"), "{text}");
    }

    #[test]
    fn answers_also_come_as_structured_content() {
        let (_dir, mut server) = server();
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_secrets", "arguments": {}}});
        let reply = server.handle_line(&line.to_string()).unwrap();
        assert!(reply["result"]["structuredContent"]["secrets"].is_array(), "{reply}");
    }

    #[test]
    fn the_action_list_gives_the_plugin_ids_other_tools_need() {
        let (_dir, mut server) = server();
        let (_, text) = call(&mut server, "list_actions", json!({}));
        assert!(text.contains("\"plugin_id\": \"test.fake\""), "{text}");

        let (failed, text) = call(&mut server, "request_site_access", json!({"plugin": "test.nothing", "host": "a.example", "reason": "x"}));
        assert!(failed, "{text}");
        let (failed, text) = call(&mut server, "request_site_access", json!({"plugin": "test.fake", "host": "a.example", "reason": "x"}));
        assert!(!failed, "{text}");
    }

    #[test]
    fn every_guide_can_be_read() {
        let (_dir, mut server) = server();
        for (topic, guide) in GUIDES {
            let (failed, text) = call(&mut server, "get_guide", json!({"topic": topic}));
            assert!(!failed, "{text}");
            assert!(guide.len() > 200 && text.contains(guide.lines().next().unwrap()), "{topic}");
        }
        let (failed, text) = call(&mut server, "get_guide", json!({"topic": "cooking"}));
        assert!(failed && text.contains("plugins"), "{text}");
    }

    #[test]
    fn a_key_can_be_moved_with_its_approval_and_what_is_inside_it() {
        let (_dir, mut server) = server();
        call(&mut server, "set_key", json!({"row": 2, "col": 0, "steps": [{"action": "test.fake/risky", "settings": {"command": "ls"}}]}));
        let active = server.ws.settings.active_profile.clone();
        let profile = server.ws.profiles.get_mut(&active).unwrap();
        profile.pages[0].key_at_mut(2, 0).unwrap().approve();
        server.ws.save_profile(&active).unwrap();

        let (failed, text) = call(&mut server, "move_key", json!({"row": 2, "col": 0, "to_row": 2, "to_col": 3}));
        assert!(!failed, "{text}");
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 3}));
        assert!(text.contains("\"ok\": true"), "still approved: {text}");

        call(&mut server, "set_key", json!({"row": 2, "col": 1, "is_folder": true, "title": "Box"}));
        call(&mut server, "set_key", json!({"row": 0, "col": 1, "folder": [[2, 1]], "title": "Inside", "steps": []}));
        let (failed, text) = call(&mut server, "move_key", json!({"row": 2, "col": 1, "to_row": 2, "to_col": 2}));
        assert!(!failed, "{text}");
        let (failed, text) = call(&mut server, "press_key", json!({"row": 0, "col": 1, "folder": [[2, 2]]}));
        assert!(!failed, "{text}");

        let (failed, text) = call(&mut server, "move_key", json!({"row": 2, "col": 2, "to_row": 2, "to_col": 3}));
        assert!(failed && text.contains("already a key"), "{text}");
        let (failed, text) = call(&mut server, "move_key", json!({"row": 2, "col": 2, "to_row": 99, "to_col": 0}));
        assert!(failed && text.contains("grid"), "{text}");
        let (failed, text) = call(&mut server, "move_key", json!({"row": 0, "col": 1, "folder": [[2, 2]], "to_row": 0, "to_col": 0}));
        assert!(failed && text.contains("back key"), "{text}");
    }

    #[test]
    fn setting_a_key_never_throws_away_a_folder_full_of_keys() {
        let (_dir, mut server) = server();
        call(&mut server, "set_key", json!({"row": 2, "col": 1, "is_folder": true}));
        // Still empty, so there's nothing to lose.
        let (failed, _) = call(&mut server, "set_key", json!({"row": 2, "col": 1, "steps": []}));
        assert!(!failed);

        call(&mut server, "set_key", json!({"row": 2, "col": 1, "is_folder": true}));
        call(&mut server, "set_key", json!({"row": 0, "col": 1, "folder": [[2, 1]], "title": "Inside", "steps": []}));
        let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 1, "steps": []}));
        assert!(failed && text.contains("nothing was changed"), "{text}");
        let (_, text) = call(&mut server, "get_profile", json!({}));
        assert!(text.contains("Inside"), "{text}");
    }

    #[test]
    fn folders_stop_at_the_deepest_the_app_can_load() {
        let (dir, mut server) = server();
        let mut path: Vec<Value> = Vec::new();
        for level in 0..MAX_FOLDER_DEPTH {
            let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 1, "is_folder": true, "folder": path, "title": "Down"}));
            assert!(!failed, "level {level}: {text}");
            path.push(json!([2, 1]));
        }
        let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 1, "is_folder": true, "folder": path}));
        assert!(failed && text.contains("deep"), "{text}");
        let again = Workspace::load(Paths::at(dir.path())).unwrap();
        assert!(again.errors.is_empty(), "{:?}", again.errors);
    }

    #[test]
    fn a_colour_the_app_cant_read_is_refused() {
        let (_dir, mut server) = server();
        for name in ["color", "text_color", "alt_color"] {
            let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 0, name: "red", "steps": []}));
            assert!(failed && text.contains(name), "{text}");
        }
        let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 0, "text_color": "#FFFFFF", "steps": []}));
        assert!(!failed, "{text}");
        let (_, text) = call(&mut server, "get_profile", json!({}));
        assert!(text.contains("#FFFFFF"), "{text}");
    }

    #[test]
    fn check_config_says_where_a_key_in_a_folder_is() {
        let (_dir, mut server) = server();
        call(&mut server, "set_key", json!({"row": 2, "col": 1, "is_folder": true}));
        call(&mut server, "set_key", json!({"row": 0, "col": 1, "folder": [[2, 1]], "title": "Needs OK", "steps": [{"action": "test.fake/risky"}]}));
        let (_, text) = call(&mut server, "check_config", json!({}));
        let report: Value = serde_json::from_str(&text).unwrap();
        let problem = report["key_problems"].as_array().unwrap().iter().find(|p| p["title"] == "Needs OK").unwrap();
        assert_eq!(problem["folder"], json!([[2, 1]]));
        assert_eq!(problem["col"], 1);
    }

    #[test]
    fn pressing_a_key_says_when_a_page_change_cant_happen_from_here() {
        let (_dir, mut server) = server();
        call(&mut server, "set_key", json!({"row": 2, "col": 0, "steps": [{"action": "core.navigation/next_page"}]}));
        let (_, text) = call(&mut server, "press_key", json!({"row": 2, "col": 0}));
        assert!(text.contains("only works from"), "{text}");
    }

    #[test]
    fn profiles_pages_and_presets_can_be_managed() {
        let (dir, mut server) = server();
        let (failed, text) = call(&mut server, "set_active_profile", json!({"profile": "nothing"}));
        assert!(failed, "{text}");
        call(&mut server, "create_profile", json!({"name": "Studio"}));
        let (failed, text) = call(&mut server, "set_active_profile", json!({"profile": "studio"}));
        assert!(!failed, "{text}");
        assert!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap().contains("active_profile = \"studio\""));

        let (failed, text) = call(&mut server, "rename_page", json!({"page": 0, "name": "Front"}));
        assert!(!failed, "{text}");
        assert_eq!(server.ws.profiles["studio"].pages[0].name, "Front");
        let (failed, _) = call(&mut server, "rename_page", json!({"page": 7, "name": "No"}));
        assert!(failed);

        call(&mut server, "add_preset", json!({"name": "Try", "action": "test.fake/call"}));
        assert_eq!(server.ws.settings.presets.len(), 1);
        let (_, text) = call(&mut server, "remove_preset", json!({"name": "Try"}));
        assert!(text.contains("true"), "{text}");
        assert!(server.ws.settings.presets.is_empty());
        let (_, text) = call(&mut server, "remove_preset", json!({"name": "Try"}));
        assert!(text.contains("false"), "{text}");
    }

    #[test]
    fn a_picture_can_be_added_listed_and_used_as_a_background() {
        let (dir, mut server) = server();
        let (_, text) = call(&mut server, "list_images", json!({}));
        assert!(text.contains("[]"), "{text}");

        let (failed, text) = call(&mut server, "add_image", json!({"path": write_png(dir.path())}));
        assert!(!failed, "{text}");
        let added: Value = serde_json::from_str(&text).unwrap();
        let name = added["background"].as_str().unwrap();
        let (_, text) = call(&mut server, "list_images", json!({}));
        assert!(text.contains(name), "{text}");
        let (failed, text) = call(&mut server, "set_key", json!({"row": 2, "col": 0, "background": name, "steps": []}));
        assert!(!failed, "{text}");

        let not_png = dir.path().join("notes.txt");
        std::fs::write(&not_png, "private notes").unwrap();
        let (failed, text) = call(&mut server, "add_image", json!({"path": not_png}));
        assert!(failed && text.contains("isn't a PNG"), "{text}");
        assert!(!text.contains("private notes"), "{text}");
    }

    #[test]
    fn settings_can_change_but_not_the_ones_that_guard_the_person() {
        let (dir, mut server) = server();
        let (failed, text) = call(&mut server, "update_settings", json!({
            "theme": "light", "accent": "#112233", "scale": 1.25, "tray_icon": "white",
            "brightness": 40, "deck_enabled": false, "run_in_background": false
        }));
        assert!(!failed, "{text}");
        let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        for setting in ["theme = \"light\"", "#112233", "scale = 1.25", "tray_icon = \"white\"", "brightness = 40", "run_in_background = false"] {
            assert!(saved.contains(setting), "{setting}: {saved}");
        }
        let (_, text) = call(&mut server, "get_settings", json!({}));
        assert!(text.contains("\"brightness\": 40"), "{text}");

        // A wrong value anywhere means nothing at all changes.
        for bad in [
            json!({"theme": "neon", "brightness": 10}),
            json!({"scale": 9.0, "brightness": 10}),
            json!({"tray_icon": "pink", "brightness": 10}),
            json!({"accent": "red", "brightness": 10}),
            json!({"brightness": 500}),
            json!({"brightness": -1}),
            json!({}),
        ] {
            let (failed, text) = call(&mut server, "update_settings", bad);
            assert!(failed, "{text}");
            assert_eq!(server.ws.settings.hardware.brightness, 40);
        }

        for guarded in ["mcp_may_use_secrets", "read_only", "enabled", "auto_lock_minutes"] {
            let (failed, text) = call(&mut server, "update_settings", json!({guarded: true}));
            assert!(failed && text.contains("no argument"), "{guarded}: {text}");
        }
        assert!(!server.ws.settings.security.mcp_may_use_secrets);
    }

    #[test]
    fn a_secret_is_only_asked_for_when_its_needed_and_its_name_is_usable() {
        let (_dir, mut server) = server();
        let (failed, text) = call(&mut server, "request_secret", json!({"name": "bad}}name", "hosts": ["a.example"], "reason": "x"}));
        assert!(failed, "{text}");
        assert!(Pending::load(&server.ws.paths).secrets.is_empty());

        {
            let mut vault = server.engine.vault.lock().unwrap();
            vault.create("a long enough passphrase").unwrap();
            vault.set("held", "value", &["a.example".into()]).unwrap();
        }
        let (failed, text) = call(&mut server, "request_secret", json!({"name": "held", "hosts": ["a.example"], "reason": "x"}));
        assert!(!failed && text.contains("{{secret:held}}") && text.contains("already"), "{text}");
        assert!(Pending::load(&server.ws.paths).secrets.is_empty());

        // For a new site, only the person can extend where it may go.
        let (failed, text) = call(&mut server, "request_secret", json!({"name": "held", "hosts": ["b.example"], "reason": "x"}));
        assert!(!failed, "{text}");
        assert_eq!(Pending::load(&server.ws.paths).secrets[0].name, "held");
    }

    #[test]
    fn serves_over_a_stream_until_it_closes() {
        let (_dir, server) = server();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n\n".to_vec();
        let mut output = Vec::new();
        server.serve(std::io::Cursor::new(input), &mut output).unwrap();
        let reply: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(reply["id"], 7);
    }

    #[test]
    fn a_client_that_sends_nonsense_does_not_stop_the_server() {
        let (_dir, server) = server();
        // Bytes that aren't text, then a message that is.
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\xff\n");
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        server.serve(std::io::Cursor::new(input), &mut output).unwrap();
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("\"id\":2"), "{text}");
    }

    #[test]
    fn an_oversized_message_is_refused_and_the_next_one_is_answered() {
        let (_dir, server) = server();
        let mut input: Vec<u8> = vec![b'x'; MAX_LINE_BYTES + 10];
        input.push(b'\n');
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        server.serve(std::io::Cursor::new(input), &mut output).unwrap();
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("message too large"), "{text}");
        assert!(text.contains("\"id\":2"), "{text}");
    }

    #[test]
    fn a_message_with_no_method_is_answered_rather_than_ignored() {
        let (_dir, mut server) = server();
        let reply = server
            .handle_line("{\"jsonrpc\":\"2.0\",\"id\":3}")
            .expect("an answer, not silence");
        assert_eq!(reply["error"]["code"], -32600);
    }

    #[test]
    fn the_bounds_a_tool_advertises_are_the_bounds_it_enforces() {
        let (_dir, mut server) = server();
        let (failed, text) = call(&mut server, "create_profile", json!({"name": "Big", "rows": 99}));
        assert!(failed, "{text}");
        assert!(text.contains("at most 8"), "{text}");
        let (failed, text) = call(&mut server, "get_guide", json!({"topic": "cooking"}));
        assert!(failed, "{text}");
        assert!(text.contains("should be one of"), "{text}");
    }
}
