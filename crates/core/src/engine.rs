//! Finds plugins and runs keys, with every check in one place.

use pyroporch_sdk::{
    ActionSpec, DeckControl, Field, FieldKind, HostHandle, Navigate, Outcome, PaintRequest, Plugin, PluginInfo, Risk, RunRequest,
    SaverTiming, Trigger,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::config::{Key, Step};
use crate::paths::{Paths, audit};
use crate::policy::{Grants, PolicyHost, authority_of};
use crate::sample_plugin::is_sample;
use crate::security::{general_findings, plaintext_findings, sha256_hex};
use crate::vault::Vault;
use crate::wasm::{MAX_WASM_BYTES, WasmPlugin};

/// A third-party plugin's `plugin.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub wasm: String,
    pub permissions: Permissions,
}

/// What a plugin asks for. Nothing is given until the person says yes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub http_hosts: Vec<String>,
    /// `*` asks for any secret the person chooses. Each still only goes to its own sites.
    pub secrets: Vec<String>,
    /// Asks to change the deck's brightness and play on its keys.
    pub deck: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub builtin: bool,
    /// One of the plugins that came with the app.
    pub sample: bool,
    /// Its folder or file in the plugins folder, for one that's there.
    pub folder: String,
    pub enabled: bool,
    /// Plain words about why it isn't running, if it isn't.
    pub note: String,
    pub requested: Permissions,
    pub sha256: String,
    pub http_hosts: Vec<String>,
}

struct Loaded {
    info: Arc<PluginInfo>,
    plugin: Arc<dyn Plugin>,
    /// The fingerprint of the file it came from, so an unchanged one can be kept.
    sha256: String,
    /// May refer to any secret, though each still only goes to its own sites.
    any_secret: bool,
    /// May change the deck and play on its keys.
    deck_allowed: bool,
}

/// The longest icon a plugin may set. Some emoji take several characters.
const MAX_LIVE_ICON_CHARS: usize = 32;

/// The action's description, without copying it. The caller has to hold the lock.
fn find_spec<'a>(loaded: &'a [Loaded], full_id: &str) -> Option<&'a ActionSpec> {
    let (plugin, action) = full_id.split_once('/')?;
    loaded
        .iter()
        .find(|p| p.info.id == plugin)?
        .info
        .actions
        .iter()
        .find(|a| a.id == action)
}

/// Mixes a number into a stamp, so a change to it is noticed.
fn mix(stamp: &mut u64, value: u64) {
    *stamp = stamp.wrapping_mul(31).wrapping_add(value);
}

/// Mixes a file's size and time into a stamp, so a change to it is noticed.
fn mix_file(stamp: &mut u64, path: &std::path::Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        *stamp = stamp.wrapping_mul(31).wrapping_add(meta.len());
        if let Some(at) = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()) {
            *stamp = stamp.wrapping_mul(31).wrapping_add(at.as_nanos() as u64);
        }
    }
}

/// Everything that stops one step's settings from being saved or run.
fn check_step_in(loaded: &[Loaded], step: &Step) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let spec = match find_spec(loaded, &step.action) {
        Some(spec) => spec,
        None => {
            // The action isn't installed, so its own form is unknown, but the general rules
            // still apply: a setting called "password" holds a password whatever it does.
            blocks.push(Block::UnknownAction(step.action.clone()));
            blocks.extend(general_findings(&step.settings).into_iter().map(Block::PlainCredential));
            return blocks;
        }
    };
    blocks.extend(
        plaintext_findings(Some(spec), &step.settings)
            .into_iter()
            .map(Block::PlainCredential),
    );
    for field in spec.fields.iter().filter(|f| f.required) {
        let missing = match step.settings.get(&field.key) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(text)) => text.is_empty(),
            Some(_) => false,
        };
        if missing {
            blocks.push(Block::MissingSetting(field.label.clone()));
        }
    }
    blocks
}

/// Whether a plugin may put this on a key. A key's own icon can name a picture file, but this comes
/// from a sandboxed plugin, which has no business opening files on the computer.
fn is_live_icon(icon: &str) -> bool {
    (1..=MAX_LIVE_ICON_CHARS).contains(&icon.chars().count()) && !icon.chars().any(|c| matches!(c, '/' | '\\' | '.') || c.is_control())
}

/// Whether this is a colour like `#2E8B57`.
fn is_color(color: &str) -> bool {
    color.strip_prefix('#').is_some_and(|hex| hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// One frame of an animation for the whole deck.
#[derive(Debug, Clone, PartialEq)]
pub struct SaverFrame {
    pub rows: u32,
    pub cols: u32,
    /// How many squares across and down each key.
    pub blocks: u32,
    /// How bright the deck should be, as a percent of the person's setting.
    pub brightness_percent: u8,
    /// Three bytes for each square, row by row across the whole deck.
    pub rgb: Vec<u8>,
}

impl SaverFrame {
    /// The colour of one square of one key, or black off the edge of the frame.
    pub fn color(&self, row: u32, col: u32, across: u32, down: u32) -> [u8; 3] {
        let width = (self.cols * self.blocks) as usize;
        let (x, y) = ((col * self.blocks + across) as usize, (row * self.blocks + down) as usize);
        if x >= width {
            return [0, 0, 0];
        }
        let at = (y * width + x) * 3;
        self.rgb.get(at..at + 3).map_or([0, 0, 0], |color| [color[0], color[1], color[2]])
    }
}

/// Why a key won't run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Block {
    UnknownAction(String),
    NeedsApproval,
    PlainCredential(String),
    MissingSetting(String),
}

impl std::fmt::Display for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Block::UnknownAction(action) => write!(f, "this key uses {action}, which isn't installed"),
            Block::NeedsApproval => write!(f, "this key acts on your computer and needs your approval in the app"),
            Block::PlainCredential(finding) => write!(f, "{finding}"),
            Block::MissingSetting(name) => write!(f, "this key still needs: {name}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub ok: bool,
    pub message: String,
    pub title: Option<String>,
    /// An icon and a colour to show on the key until its next run.
    pub icon: Option<String>,
    pub color: Option<String>,
    /// Every page or profile change, in the order they happened.
    pub navigate: Vec<Navigate>,
    /// Every change to the deck, in the order they happened.
    pub deck: Vec<DeckControl>,
}

/// Something a step wants done to the deck or the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Navigate(Navigate),
    Deck(DeckControl),
    /// A key asked for a screen saver: on, or off if it's the key that turned it on.
    ScreenSaver {
        plugin: String,
        settings: serde_json::Map<String, serde_json::Value>,
        timing: SaverTiming,
    },
}

/// Page and profile changes, offered as an ordinary plugin.
struct NavigationPlugin;
pub const NAVIGATION_ID: &str = "core.navigation";

impl Plugin for NavigationPlugin {
    fn info(&self) -> PluginInfo {
        fn action(id: &str, name: &str, icon: &str, fields: Vec<Field>) -> ActionSpec {
            ActionSpec {
                id: id.into(),
                name: name.into(),
                description: String::new(),
                icon: icon.into(),
                risk: Risk::Low,
                refreshable: false,
                hidden: false,
                previews: false,
                fields,
            }
        }
        PluginInfo {
            id: NAVIGATION_ID.into(),
            name: "Navigation".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Move between pages and profiles.".into(),
            actions: vec![
                action("next_page", "Next Page", "➡", vec![]),
                action("previous_page", "Previous Page", "⬅", vec![]),
                action("go_to_page", "Go to Page", "📄", vec![Field::new("page", "Page name", FieldKind::Text).required()]),
                action("switch_profile", "Switch Profile", "🔀", vec![Field::new("profile", "Profile id", FieldKind::Text).required()]),
                // Each step of a multi-step key has its own wait, so this adds nothing to the list.
                // It stays so that keys already using it keep working.
                ActionSpec {
                    hidden: true,
                    ..action("wait", "Wait", "⏱", vec![Field::new("seconds", "Seconds", FieldKind::Number).default(1)])
                },
            ],
        }
    }

    fn run(&self, request: &RunRequest, _host: &HostHandle) -> Result<Outcome, String> {
        let navigate = match request.action.as_str() {
            "next_page" => Navigate::NextPage,
            "previous_page" => Navigate::PreviousPage,
            "go_to_page" => Navigate::Page(request.text("page").into()),
            "switch_profile" => Navigate::Profile(request.text("profile").into()),
            "wait" => {
                let seconds = request.number("seconds").unwrap_or(1.0).clamp(0.0, 600.0);
                std::thread::sleep(Duration::from_secs_f64(seconds));
                return Ok(Outcome::message("waited"));
            }
            other => return Err(format!("unknown action: {other}")),
        };
        Ok(Outcome {
            navigate: Some(navigate),
            ..Outcome::default()
        })
    }
}

/// The drawn icon for the sleep key: a lantern that's gone out.
pub const SLEEP_ICON: &str = "builtin:lantern-out";
/// The name that icon had before, which keys saved back then still hold.
pub const OLD_SLEEP_ICON: &str = "builtin:campfire-out";
const DEFAULT_BRIGHTNESS_STEP: f64 = 10.0;

/// Brightness and sleep for the physical deck, offered as an ordinary plugin.
struct DeckPlugin;
pub const DECK_ID: &str = "core.deck";

impl Plugin for DeckPlugin {
    fn info(&self) -> PluginInfo {
        fn action(id: &str, name: &str, description: &str, icon: &str, fields: Vec<Field>) -> ActionSpec {
            ActionSpec {
                id: id.into(),
                name: name.into(),
                description: description.into(),
                icon: icon.into(),
                risk: Risk::Low,
                refreshable: false,
                hidden: false,
                previews: false,
                fields,
            }
        }
        let step = || Field::new("step", "Change by (%)", FieldKind::Number).default(DEFAULT_BRIGHTNESS_STEP);
        PluginInfo {
            id: DECK_ID.into(),
            name: "Deck".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Brightness, and sleep with every key off.".into(),
            actions: vec![
                action("sleep", "Sleep", "Turn every key off. Press any key to wake it.", SLEEP_ICON, vec![]),
                action("brighter", "Brighter", "Turn the brightness up.", "🔆", vec![step()]),
                action("dimmer", "Dimmer", "Turn the brightness down.", "🔅", vec![step()]),
                action("set_brightness", "Set Brightness", "Go straight to one brightness.", "☀", vec![
                    Field::new("percent", "Brightness (%)", FieldKind::Number).default(50),
                ]),
            ],
        }
    }

    fn run(&self, request: &RunRequest, _host: &HostHandle) -> Result<Outcome, String> {
        let percent = |key: &str, fallback: f64| request.number(key).unwrap_or(fallback).clamp(0.0, 100.0) as u8;
        let deck = match request.action.as_str() {
            "sleep" => DeckControl::Sleep,
            "brighter" => DeckControl::Brighter(percent("step", DEFAULT_BRIGHTNESS_STEP)),
            "dimmer" => DeckControl::Dimmer(percent("step", DEFAULT_BRIGHTNESS_STEP)),
            "set_brightness" => DeckControl::SetBrightness(percent("percent", 50.0)),
            other => return Err(format!("unknown action: {other}")),
        };
        Ok(Outcome {
            deck: Some(deck),
            ..Outcome::default()
        })
    }
}

pub struct Engine {
    pub paths: Paths,
    pub vault: Arc<Mutex<Vault>>,
    natives: Vec<Arc<dyn Plugin>>,
    loaded: RwLock<Vec<Loaded>>,
    statuses: RwLock<Vec<PluginStatus>>,
    /// What the plugins folder looked like last time it was read, so it isn't read again
    /// for nothing.
    folder_stamp: Mutex<Option<u64>>,
}

fn read_manifest(dir: &std::path::Path) -> Result<Manifest, String> {
    if !dir.join("plugin.toml").is_file() {
        return Err("it isn't a plain file".into());
    }
    let text = std::fs::read_to_string(dir.join("plugin.toml")).map_err(|e| e.to_string())?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    if manifest.id.is_empty() || manifest.wasm.is_empty() {
        return Err("plugin.toml needs an id and a wasm file".into());
    }
    if manifest.id.contains(['/', ' ']) {
        return Err("the id can't contain a slash or a space".into());
    }
    let simple_name = !manifest.wasm.contains(['/', '\\']) && manifest.wasm != "..";
    if !simple_name {
        return Err("the wasm file must sit next to plugin.toml".into());
    }
    Ok(manifest)
}

fn not_a_plugin(folder: &str, note: String) -> PluginStatus {
    PluginStatus {
        id: folder.to_string(),
        name: folder.to_string(),
        version: String::new(),
        description: String::new(),
        builtin: false,
        sample: false,
        folder: folder.to_string(),
        enabled: false,
        note,
        requested: Permissions::default(),
        sha256: String::new(),
        http_hosts: Vec::new(),
    }
}

/// Says what's in a folder or file that has no `plugin.toml`, and what to do about it.
fn why_not_a_plugin(path: &std::path::Path, builtin_ids: &[String]) -> Option<String> {
    let has_extension = |path: &std::path::Path, extension: &str| {
        path.extension().is_some_and(|found| found.eq_ignore_ascii_case(extension))
    };
    if path.is_file() {
        return has_extension(path, "wasm").then(|| {
            "This plugin file needs its own folder here, next to a plugin.toml that says what it asks for.".to_string()
        });
    }
    if path.join("Cargo.toml").is_file()
        && let Ok(text) = std::fs::read_to_string(path.join("Cargo.toml"))
    {
        let name = text
            .parse::<toml::Table>()
            .ok()
            .and_then(|table| table.get("package")?.get("name")?.as_str().map(str::to_string))
            .unwrap_or_default();
        let builtin = name.strip_prefix("pyroporch-").map(|short| format!("pyroporch.{short}"));
        if builtin.is_some_and(|id| builtin_ids.contains(&id)) {
            return Some("This is the source code of a module that's already built into the app, so there's nothing to add. You can take this folder out.".into());
        }
        return Some("This is source code, and it has to be built first. Build it for wasm32-unknown-unknown, then put the .wasm file here in a folder with a plugin.toml.".into());
    }
    let holds_plugin_file = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| has_extension(&entry.path(), "wasm"));
    Some(if holds_plugin_file {
        "This folder has a plugin file but no plugin.toml, which is what says what the plugin asks for.".into()
    } else {
        "This folder has no plugin.toml, so it isn't a plugin.".into()
    })
}

impl Engine {
    pub fn new(paths: Paths, vault: Vault, mut natives: Vec<Arc<dyn Plugin>>) -> Self {
        natives.insert(0, Arc::new(DeckPlugin));
        natives.insert(0, Arc::new(NavigationPlugin));
        let engine = Self {
            paths,
            vault: Arc::new(Mutex::new(vault)),
            natives,
            loaded: RwLock::new(Vec::new()),
            statuses: RwLock::new(Vec::new()),
            folder_stamp: Mutex::new(None),
        };
        engine.refresh_plugins();
        engine
    }

    /// Looks at the plugins folder again, but only if something in it has changed. Reading it
    /// means hashing every plugin's file, so it isn't done for nothing.
    pub fn refresh_plugins_if_changed(&self) {
        let stamp = self.plugins_stamp();
        // The lock is let go before refreshing, because refreshing takes it too.
        let unchanged = match self.folder_stamp.lock() {
            Ok(slot) => *slot == Some(stamp),
            Err(_) => return,
        };
        if !unchanged {
            self.refresh_plugins();
        }
    }

    /// What the plugins folder and the person's allowances look like right now.
    fn plugins_stamp(&self) -> u64 {
        let mut stamp: u64 = 0;
        if let Ok(entries) = std::fs::read_dir(self.paths.plugins_dir()) {
            for entry in entries.flatten() {
                mix(&mut stamp, entry.file_name().to_string_lossy().len() as u64);
                mix_file(&mut stamp, &entry.path());
                // A plugin's own files: replacing one of them changes nothing else.
                if entry.path().is_dir()
                    && let Ok(files) = std::fs::read_dir(entry.path())
                {
                    for file in files.flatten() {
                        mix(&mut stamp, file.file_name().to_string_lossy().len() as u64);
                        mix_file(&mut stamp, &file.path());
                    }
                }
            }
        }
        mix_file(&mut stamp, &self.paths.grants());
        stamp
    }

    /// Looks at the plugins folder again and loads what the person has approved.
    pub fn refresh_plugins(&self) {
        // A grants file with a mistake allows nothing, and the cards say why.
        let (grants, grants_broken) = match Grants::try_load(&self.paths) {
            Ok(grants) => (grants, false),
            Err(_) => (Grants::default(), true),
        };
        let mut loaded = Vec::new();
        let mut statuses = Vec::new();
        // A plugin whose file hasn't changed keeps the copy that was already loaded, so an
        // animation isn't restarted every time the folder is looked at.
        let previous: BTreeMap<String, (String, Arc<dyn Plugin>)> = self.loaded.read().map_or_else(
            |_| BTreeMap::new(),
            |list| {
                list.iter()
                    .filter(|p| !p.sha256.is_empty())
                    .map(|p| (p.info.id.clone(), (p.sha256.clone(), p.plugin.clone())))
                    .collect()
            },
        );
        for plugin in &self.natives {
            let info = plugin.info();
            statuses.push(PluginStatus {
                id: info.id.clone(),
                name: info.name.clone(),
                version: info.version.clone(),
                description: info.description.clone(),
                builtin: true,
                sample: false,
                folder: String::new(),
                enabled: true,
                note: String::new(),
                requested: Permissions::default(),
                sha256: String::new(),
                http_hosts: grants.plugins.get(&info.id).map_or_else(Vec::new, |g| g.http_hosts.clone()),
            });
            loaded.push(Loaded {
                info: Arc::new(info),
                plugin: plugin.clone(),
                sha256: String::new(),
                any_secret: true,
                deck_allowed: true,
            });
        }

        let entries = std::fs::read_dir(self.paths.plugins_dir()).into_iter().flatten().flatten();
        let mut found: Vec<_> = entries
            .map(|e| e.path())
            // Starting with a dot or an underscore marks something as not a plugin to load.
            .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(['.', '_'])))
            .collect();
        found.sort();
        let builtin_ids: Vec<String> = self.natives.iter().map(|n| n.info().id).collect();
        for dir in found {
            let folder = dir.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            // Anything here that isn't a plugin still gets a card saying what's wrong, because
            // otherwise it would look as if the app hadn't noticed.
            if !dir.join("plugin.toml").exists() {
                if let Some(note) = why_not_a_plugin(&dir, &builtin_ids) {
                    statuses.push(not_a_plugin(&folder, note));
                }
                continue;
            }
            let manifest = match read_manifest(&dir) {
                Ok(manifest) => manifest,
                Err(e) => {
                    statuses.push(not_a_plugin(&folder, format!("Can't read its plugin.toml: {e}")));
                    continue;
                }
            };
            let mut status = PluginStatus {
                id: manifest.id.clone(),
                name: if manifest.name.is_empty() { manifest.id.clone() } else { manifest.name.clone() },
                version: manifest.version.clone(),
                description: manifest.description.clone(),
                builtin: false,
                sample: is_sample(&manifest.id),
                folder: folder.clone(),
                enabled: false,
                note: String::new(),
                requested: manifest.permissions.clone(),
                sha256: String::new(),
                http_hosts: Vec::new(),
            };
            if self.natives.iter().any(|n| n.info().id == manifest.id) || statuses.iter().any(|s| s.id == manifest.id) {
                status.note = "Its id is already taken, so it was ignored.".into();
                statuses.push(status);
                continue;
            }
            // These names say the app itself vouches for something, so a plugin can't take one.
            if (manifest.id.starts_with("core.") || manifest.id.starts_with("pyroporch.")) && !is_sample(&manifest.id) {
                status.note = "Its id starts with a name kept for the app itself, so it was ignored.".into();
                statuses.push(status);
                continue;
            }
            let path = dir.join(&manifest.wasm);
            let bytes = match std::fs::metadata(&path) {
                // Reading a pipe or a device could wait or go on forever, before any approval.
                Ok(meta) if !meta.is_file() => Err(format!("{} isn't a plain file.", manifest.wasm)),
                Ok(meta) if meta.len() as usize > MAX_WASM_BYTES => Err("The plugin file is too big.".to_string()),
                Ok(_) => std::fs::read(&path).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Can't read {}: {e}", manifest.wasm)),
            };
            let bytes = match bytes {
                Ok(bytes) => bytes,
                Err(e) => {
                    status.note = e;
                    statuses.push(status);
                    continue;
                }
            };
            status.sha256 = sha256_hex(&bytes);
            let grant = grants.plugins.get(&manifest.id).cloned().unwrap_or_default();
            status.http_hosts = grant.http_hosts.clone();
            if grants_broken {
                status.note = "grants.toml has a mistake, so this stays off until it's fixed.".into();
            } else if !grant.enabled {
                status.note = "Waiting for your approval.".into();
            } else if grant.sha256 != status.sha256 {
                status.note = "The plugin file changed since you approved it, so it needs approving again.".into();
            } else {
                let any_secret = grant.secrets.iter().any(|name| name == "*");
                let kept = previous
                    .get(&manifest.id)
                    .filter(|(hash, _)| *hash == status.sha256)
                    .map(|(_, plugin)| plugin.clone());
                match kept {
                    // The file is the same one that was approved, so the copy that is already
                    // running is kept, with whatever it has going on.
                    Some(plugin) => {
                        status.enabled = true;
                        let mut info = plugin.info();
                        // Sandboxed code can only do what its grants allow, whatever it claims.
                        for action in &mut info.actions {
                            action.risk = Risk::Low;
                        }
                        loaded.push(Loaded {
                            info: Arc::new(info),
                            plugin,
                            sha256: status.sha256.clone(),
                            any_secret,
                            deck_allowed: grant.deck,
                        });
                    }
                    None => match WasmPlugin::load(&bytes) {
                        Ok(plugin) if plugin.info().id == manifest.id => {
                            status.enabled = true;
                            let mut info = plugin.info();
                            // Sandboxed code can only do what its grants allow, whatever it claims.
                            for action in &mut info.actions {
                                action.risk = Risk::Low;
                            }
                            loaded.push(Loaded {
                                info: Arc::new(info),
                                plugin: Arc::new(plugin),
                                sha256: status.sha256.clone(),
                                any_secret,
                                deck_allowed: grant.deck,
                            });
                        }
                        Ok(_) => status.note = "The file's id doesn't match its plugin.toml.".into(),
                        Err(e) => status.note = e,
                    },
                }
            }
            statuses.push(status);
        }
        if let Ok(mut slot) = self.loaded.write() {
            *slot = loaded;
        }
        if let Ok(mut slot) = self.statuses.write() {
            *slot = statuses;
        }
        // Remember what the folder looked like, so it isn't read again for nothing.
        let stamp = self.plugins_stamp();
        if let Ok(mut slot) = self.folder_stamp.lock() {
            *slot = Some(stamp);
        }
    }

    pub fn plugin_statuses(&self) -> Vec<PluginStatus> {
        self.statuses.read().map_or_else(|_| Vec::new(), |s| s.clone())
    }

    /// Every running plugin's description.
    pub fn plugin_infos(&self) -> Vec<Arc<PluginInfo>> {
        self.loaded
            .read()
            .map_or_else(|_| Vec::new(), |l| l.iter().map(|p| p.info.clone()).collect())
    }

    /// The person approves a third-party plugin exactly as it is now, with what it asked for.
    pub fn approve_plugin(&self, id: &str) -> Result<(), String> {
        let status = self
            .plugin_statuses()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or("no such plugin")?;
        let mut grants = Grants::try_load(&self.paths)?;
        let grant = grants.plugins.entry(id.to_string()).or_default();
        grant.enabled = true;
        grant.sha256 = status.sha256;
        // Sites the person already allowed for this plugin stay allowed. Unencrypted traffic is
        // only ever allowed by editing the file, so approving can't grant it.
        for host in status.requested.http_hosts {
            let host = host.strip_prefix("http://").unwrap_or(&host).to_string();
            if !grant.http_hosts.contains(&host) {
                grant.http_hosts.push(host);
            }
        }
        grant.secrets = status.requested.secrets;
        grant.deck = status.requested.deck;
        grants.save(&self.paths)?;
        audit(&self.paths, "person", &format!("approved plugin {id}"));
        self.refresh_plugins();
        Ok(())
    }

    pub fn disable_plugin(&self, id: &str) -> Result<(), String> {
        let mut grants = Grants::try_load(&self.paths)?;
        grants.plugins.entry(id.to_string()).or_default().enabled = false;
        grants.save(&self.paths)?;
        audit(&self.paths, "person", &format!("turned off plugin {id}"));
        self.refresh_plugins();
        Ok(())
    }

    /// Takes a plugin out of the plugins folder. Sites the person allowed are remembered, in case
    /// it comes back, but its approval isn't. The folder says which one is meant, because a copied
    /// plugin has the same id as the one it was copied from.
    pub fn remove_plugin(&self, id: &str, folder: &str) -> Result<(), String> {
        let statuses = self.plugin_statuses();
        let status = statuses
            .iter()
            .find(|s| s.id == id && s.folder == folder && !s.builtin)
            .ok_or("that isn't in the plugins folder")?;
        if status.folder.is_empty() || status.folder.contains(['/', '\\']) || status.folder == ".." {
            return Err("that isn't in the plugins folder".into());
        }
        // Checked first, so a file that can't be updated doesn't leave the job half done.
        let mut grants = Grants::try_load(&self.paths)?;
        let path = self.paths.plugins_dir().join(&status.folder);
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.map_err(|e| format!("couldn't remove it: {e}"))?;
        // With a copy still there, the approval stays: it only covers the exact file approved.
        let copy_left = statuses.iter().any(|s| s.id == id && s.folder != folder);
        if !copy_left && let Some(grant) = grants.plugins.get_mut(id) {
            grant.enabled = false;
            grant.sha256.clear();
            grants.save(&self.paths)?;
        }
        audit(&self.paths, "person", &format!("removed plugin {id}"));
        self.refresh_plugins();
        Ok(())
    }

    /// Asks a plugin to draw a frame of the whole deck.
    pub fn paint(&self, plugin_id: &str, request: &PaintRequest) -> Result<SaverFrame, String> {
        let plugin = {
            let loaded = self.loaded.read().map_err(|_| "plugins unavailable")?;
            let entry = loaded.iter().find(|p| p.info.id == plugin_id).ok_or("that plugin isn't running")?;
            if !entry.deck_allowed {
                return Err("that plugin wasn't given access to the deck".into());
            }
            entry.plugin.clone()
        };
        let blocks = request.blocks_per_key.max(1);
        // The size is worked out with care: a grid too big to hold would overflow, and a
        // wrapped number would let a frame of the wrong size through.
        let Some(squares) = (request.rows as usize)
            .checked_mul(blocks as usize)
            .and_then(|across| (request.cols as usize).checked_mul(blocks as usize).and_then(|down| across.checked_mul(down)))
            .and_then(|squares| squares.checked_mul(3))
        else {
            return Err("that grid is too big to draw".into());
        };
        let bytes = plugin.paint(request)?;
        if bytes.len() != 1 + squares {
            return Err("the plugin drew the wrong amount".into());
        }
        Ok(SaverFrame {
            rows: request.rows,
            cols: request.cols,
            blocks,
            brightness_percent: bytes[0].min(100),
            rgb: bytes[1..].to_vec(),
        })
    }

    pub fn spec(&self, full_id: &str) -> Option<ActionSpec> {
        let loaded = self.loaded.read().ok()?;
        find_spec(&loaded, full_id).cloned()
    }

    /// Whether an action may be re-run on a timer.
    pub fn is_refreshable(&self, full_id: &str) -> bool {
        self.loaded
            .read()
            .ok()
            .and_then(|loaded| find_spec(&loaded, full_id).map(|spec| spec.refreshable))
            .unwrap_or(false)
    }

    /// Everything that stops this step's settings from being saved or run.
    pub fn check_step(&self, step: &Step) -> Vec<Block> {
        let Ok(loaded) = self.loaded.read() else {
            return vec![Block::UnknownAction(step.action.clone())];
        };
        check_step_in(&loaded, step)
    }

    /// Everything stopping a key from running right now.
    pub fn check_key(&self, key: &Key) -> Vec<Block> {
        let Ok(loaded) = self.loaded.read() else {
            return Vec::new();
        };
        let mut blocks: Vec<Block> = Vec::new();
        let mut high_risk = false;
        for step in &key.steps {
            blocks.extend(check_step_in(&loaded, step));
            if find_spec(&loaded, &step.action).is_some_and(|spec| spec.risk == Risk::High) {
                high_risk = true;
            }
        }
        if high_risk && !key.is_approved() {
            blocks.push(Block::NeedsApproval);
        }
        blocks
    }

    /// The sites this key's steps will call, with the plugin that would call them.
    pub fn sites_called(&self, key: &Key) -> Vec<(String, String)> {
        let Ok(loaded) = self.loaded.read() else {
            return Vec::new();
        };
        let mut sites = Vec::new();
        for step in &key.steps {
            let Some(spec) = find_spec(&loaded, &step.action) else {
                continue;
            };
            let Some((plugin, _)) = step.action.split_once('/') else {
                continue;
            };
            for field in spec.fields.iter().filter(|f| f.kind == FieldKind::RequestUrl) {
                let address = step.settings.get(&field.key).and_then(|v| v.as_str()).unwrap_or("");
                let address = address.split('?').next().unwrap_or("");
                if let Some(site) = url::Url::parse(address).ok().as_ref().and_then(authority_of) {
                    sites.push((plugin.to_string(), site));
                }
            }
        }
        sites
    }

    /// Runs a key's steps in order, stopping at the first failure. Blocks until done.
    pub fn run_key(&self, key: &Key, trigger: Trigger, actor: &str, secrets_allowed: bool) -> RunReport {
        self.run_key_live(key, trigger, actor, secrets_allowed, |_| {})
    }

    /// The same, telling `live` about each effect as its step finishes, so page changes and
    /// brightness changes happen when their step does, and not all at the end.
    pub fn run_key_live(&self, key: &Key, trigger: Trigger, actor: &str, secrets_allowed: bool, mut live: impl FnMut(Effect)) -> RunReport {
        let blocks = self.check_key(key);
        if let Some(block) = blocks.first() {
            return RunReport {
                ok: false,
                message: block.to_string(),
                ..RunReport::default()
            };
        }
        if key.steps.is_empty() {
            return RunReport {
                ok: true,
                message: "this key has nothing to do yet".into(),
                ..RunReport::default()
            };
        }
        let mut report = RunReport {
            ok: true,
            ..RunReport::default()
        };
        let mut messages = Vec::new();
        // A timer only repeats what asked to be repeated, never the steps that change something.
        let steps = key.steps.iter().filter(|step| trigger == Trigger::Press || self.is_refreshable(&step.action));
        for step in steps {
            if step.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(step.delay_ms.min(600_000)));
            }
            match self.run_step(step, trigger, actor, secrets_allowed) {
                Ok(outcome) => {
                    if !outcome.message.is_empty() {
                        messages.push(outcome.message);
                    }
                    report.title = outcome.title.or(report.title);
                    report.icon = outcome.icon.or(report.icon);
                    report.color = outcome.color.or(report.color);
                    if let Some(navigate) = outcome.navigate {
                        report.navigate.push(navigate.clone());
                        live(Effect::Navigate(navigate));
                    }
                    if let Some(control) = outcome.deck {
                        report.deck.push(control);
                        match (control, step.action.split_once('/')) {
                            (DeckControl::ScreenSaver(timing), Some((plugin, _))) => live(Effect::ScreenSaver {
                                plugin: plugin.to_string(),
                                settings: step.settings.clone(),
                                timing,
                            }),
                            _ => live(Effect::Deck(control)),
                        }
                    }
                }
                Err(error) => {
                    report.ok = false;
                    messages.push(error);
                    break;
                }
            }
        }
        report.message = messages.join("; ");
        report
    }

    fn run_step(&self, step: &Step, trigger: Trigger, actor: &str, secrets_allowed: bool) -> Result<Outcome, String> {
        let (plugin_id, action) = step.action.split_once('/').ok_or("bad action name")?;
        let (plugin, any_secret, deck_allowed) = {
            let loaded = self.loaded.read().map_err(|_| "plugins unavailable")?;
            let entry = loaded.iter().find(|p| p.info.id == plugin_id).ok_or("that plugin isn't running")?;
            (entry.plugin.clone(), entry.any_secret, entry.deck_allowed)
        };
        let host: HostHandle = Arc::new(PolicyHost {
            plugin_id: plugin_id.to_string(),
            paths: self.paths.clone(),
            vault: self.vault.clone(),
            any_secret,
            secrets_allowed,
            actor: actor.to_string(),
            counts_as_use: trigger == Trigger::Press,
            grants: std::sync::OnceLock::new(),
        });
        let mut outcome = plugin.run(
            &RunRequest {
                action: action.to_string(),
                settings: step.settings.clone(),
                trigger,
            },
            &host,
        )?;
        if !deck_allowed {
            outcome.deck = None;
        }
        outcome.icon = outcome.icon.filter(|icon| is_live_icon(icon));
        outcome.color = outcome.color.filter(|color| is_color(color));
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::KdfParams;
    use serde_json::json;

    struct Dummy;
    impl Plugin for Dummy {
        fn info(&self) -> PluginInfo {
            PluginInfo {
                id: "test.dummy".into(),
                name: "Dummy".into(),
                version: "1".into(),
                description: String::new(),
                actions: vec![
                    ActionSpec {
                        id: "safe".into(),
                        name: "Safe".into(),
                        description: String::new(),
                        icon: String::new(),
                        risk: Risk::Low,
                        refreshable: false,
                        hidden: false,
                        previews: false,
                        fields: vec![Field::new("needed", "Needed thing", FieldKind::Text).required()],
                    },
                    ActionSpec {
                        id: "risky".into(),
                        name: "Risky".into(),
                        description: String::new(),
                        icon: String::new(),
                        risk: Risk::High,
                        refreshable: false,
                        hidden: false,
                        previews: false,
                        fields: vec![Field::new("command", "Command", FieldKind::Text)],
                    },
                ],
            }
        }
        fn run(&self, request: &RunRequest, _: &HostHandle) -> Result<Outcome, String> {
            Ok(Outcome::message(format!("ran {}", request.action)))
        }
    }

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        paths.ensure().unwrap();
        let vault = Vault::with_kdf(
            paths.vault(),
            KdfParams {
                memory_kib: 1024,
                passes: 1,
                lanes: 1,
            },
        )
        .unwrap();
        (dir, Engine::new(paths, vault, vec![Arc::new(Dummy)]))
    }

    fn step(action: &str, settings: serde_json::Value) -> Step {
        Step {
            action: action.into(),
            settings: settings.as_object().unwrap().clone(),
            delay_ms: 0,
        }
    }

    #[test]
    fn a_changed_plugins_folder_is_noticed_without_hanging() {
        let (dir, engine) = engine();
        let path = dir.path().join("plugins/third");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("plugin.toml"), "id = \"com.third\"\nwasm = \"p.wasm\"\n").unwrap();
        let engine = Arc::new(engine);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = engine.clone();
        std::thread::spawn(move || {
            worker.refresh_plugins_if_changed();
            let _ = sender.send(());
        });
        receiver.recv_timeout(Duration::from_secs(20)).expect("the refresh hung");
        assert!(engine.plugin_statuses().iter().any(|s| s.id == "com.third"));
    }

    #[test]
    fn removing_a_copied_plugin_leaves_the_original_and_its_approval() {
        let (dir, engine) = engine();
        for folder in ["dice", "dice-2"] {
            let path = dir.path().join("plugins").join(folder);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("plugin.toml"), "id = \"com.dice\"\nwasm = \"p.wasm\"\n").unwrap();
            std::fs::write(path.join("p.wasm"), b"not really").unwrap();
        }
        engine.refresh_plugins();
        engine.approve_plugin("com.dice").unwrap();
        engine.remove_plugin("com.dice", "dice-2").unwrap();
        assert!(dir.path().join("plugins/dice").exists());
        assert!(!dir.path().join("plugins/dice-2").exists());
        assert!(Grants::load(&engine.paths).plugins["com.dice"].enabled);
        assert!(engine.remove_plugin("com.dice", "somewhere-else").is_err());

        // Removing the last one does take the approval with it.
        engine.remove_plugin("com.dice", "dice").unwrap();
        assert!(!dir.path().join("plugins/dice").exists());
        assert!(!Grants::load(&engine.paths).plugins["com.dice"].enabled);
    }

    #[test]
    fn a_grants_file_with_a_mistake_stops_plugin_changes_and_says_why() {
        let (dir, engine) = engine();
        let path = dir.path().join("plugins/third");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("plugin.toml"), "id = \"com.third\"\nwasm = \"p.wasm\"\n").unwrap();
        std::fs::write(path.join("p.wasm"), b"not really").unwrap();
        let grants = dir.path().join("grants.toml");
        std::fs::write(&grants, "[plugins.a\n").unwrap();
        engine.refresh_plugins();
        let status = engine.plugin_statuses().into_iter().find(|s| s.id == "com.third").unwrap();
        assert!(status.note.contains("grants.toml"), "{}", status.note);
        assert!(engine.approve_plugin("com.third").is_err());
        assert!(engine.disable_plugin("com.third").is_err());
        assert!(engine.remove_plugin("com.third", "third").is_err());
        assert!(path.exists(), "nothing is removed if the job can't be finished");
        assert_eq!(std::fs::read_to_string(&grants).unwrap(), "[plugins.a\n");
    }

    #[test]
    fn a_plugin_cant_take_a_name_kept_for_the_app() {
        let (dir, engine) = engine();
        let path = dir.path().join("plugins/fake");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("plugin.toml"), "id = \"core.fake\"\nwasm = \"p.wasm\"\n").unwrap();
        std::fs::write(path.join("p.wasm"), b"not really").unwrap();
        engine.refresh_plugins();
        let status = engine.plugin_statuses().into_iter().find(|s| s.id == "core.fake").unwrap();
        assert!(status.note.contains("kept for the app"), "{}", status.note);
        assert!(status.sha256.is_empty(), "so it can't be approved");
    }

    struct Painter;
    impl Plugin for Painter {
        fn info(&self) -> PluginInfo {
            PluginInfo {
                id: "test.painter".into(),
                name: "Painter".into(),
                version: "1".into(),
                description: String::new(),
                actions: vec![ActionSpec { id: "look".into(), name: "Look".into(), ..ActionSpec::default() }],
            }
        }
        fn run(&self, request: &RunRequest, _: &HostHandle) -> Result<Outcome, String> {
            Ok(Outcome {
                icon: Some(request.text("icon").into()),
                color: Some(request.text("color").into()),
                ..Outcome::default()
            })
        }
    }

    #[test]
    fn a_plugin_can_change_a_keys_icon_and_colour_but_cant_make_it_open_a_file() {
        let (_dir, engine) = engine();
        let engine = Engine::new(engine.paths.clone(), Vault::with_kdf(engine.paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap(), vec![Arc::new(Painter)]);
        let run = |icon: &str, color: &str| {
            let key = Key { steps: vec![step("test.painter/look", json!({"icon": icon, "color": color}))], ..Key::default() };
            let report = engine.run_key(&key, Trigger::Press, "test", true);
            assert!(report.ok, "{}", report.message);
            (report.icon, report.color)
        };
        assert_eq!(run("🔥", "#C0392B"), (Some("🔥".into()), Some("#C0392B".into())));
        assert_eq!(run(SLEEP_ICON, "#c0392b"), (Some(SLEEP_ICON.into()), Some("#c0392b".into())));
        for icon in ["/etc/passwd", "..\\secret", "photo.png", "", "\n"] {
            assert_eq!(run(icon, "#000000").0, None, "{icon}");
        }
        for color in ["red", "#12345", "#1234567", "#GGGGGG", ""] {
            assert_eq!(run("x", color).1, None, "{color}");
        }
    }

    #[test]
    fn a_timer_only_runs_the_steps_that_asked_to_refresh() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![step("core.deck/sleep", json!({}))],
            ..Key::default()
        };
        assert_eq!(engine.run_key(&key, Trigger::Press, "test", true).deck, vec![DeckControl::Sleep]);
        assert!(engine.run_key(&key, Trigger::Refresh, "test", true).deck.is_empty());
    }

    #[test]
    fn approving_a_plugin_never_allows_unencrypted_traffic() {
        let (dir, engine) = engine();
        let path = dir.path().join("plugins/third");
        std::fs::create_dir_all(&path).unwrap();
        let manifest = "id = \"com.third\"\nwasm = \"p.wasm\"\n[permissions]\nhttp_hosts = [\"http://api.example.com\"]\n";
        std::fs::write(path.join("plugin.toml"), manifest).unwrap();
        std::fs::write(path.join("p.wasm"), b"not really").unwrap();
        engine.refresh_plugins();
        engine.approve_plugin("com.third").unwrap();
        assert_eq!(Grants::load(&engine.paths).plugins["com.third"].http_hosts, vec!["api.example.com".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_in_a_plugin_folder_is_refused_rather_than_read() {
        let (dir, engine) = engine();
        let path = dir.path().join("plugins/third");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("plugin.toml"), "id = \"com.third\"\nwasm = \"p.wasm\"\n").unwrap();
        let pipe = path.join("p.wasm");
        assert!(std::process::Command::new("mkfifo").arg(&pipe).status().unwrap().success());
        let engine = Arc::new(engine);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = engine.clone();
        std::thread::spawn(move || {
            worker.refresh_plugins();
            let _ = sender.send(());
        });
        receiver.recv_timeout(Duration::from_secs(20)).expect("reading the pipe hung");
        let status = engine.plugin_statuses().into_iter().find(|s| s.id == "com.third").unwrap();
        assert!(status.note.contains("plain file"), "{}", status.note);
    }

    #[test]
    fn a_plugin_id_with_a_slash_or_space_is_refused_with_a_reason() {
        let (dir, engine) = engine();
        for (folder, id) in [("slash", "com.example/bad"), ("space", "com example")] {
            let path = dir.path().join("plugins").join(folder);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("plugin.toml"), format!("id = \"{id}\"\nwasm = \"p.wasm\"\n")).unwrap();
        }
        engine.refresh_plugins();
        let notes: Vec<String> = engine.plugin_statuses().into_iter().map(|s| s.note).collect();
        assert_eq!(notes.iter().filter(|note| note.contains("slash or a space")).count(), 2, "{notes:?}");
    }

    #[test]
    fn runs_steps_in_order_and_navigates() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![
                step("test.dummy/safe", json!({"needed": "x"})),
                step("core.navigation/next_page", json!({})),
            ],
            ..Key::default()
        };
        let report = engine.run_key(&key, Trigger::Press, "test", true);
        assert!(report.ok, "{}", report.message);
        assert_eq!(report.message, "ran safe");
        assert_eq!(report.navigate, vec![Navigate::NextPage]);
    }

    #[test]
    fn risky_keys_need_approval_and_lose_it_on_any_change() {
        let (_dir, engine) = engine();
        let mut key = Key {
            steps: vec![step("test.dummy/risky", json!({"command": "ls"}))],
            ..Key::default()
        };
        assert_eq!(engine.check_key(&key), vec![Block::NeedsApproval]);
        assert!(!engine.run_key(&key, Trigger::Press, "test", true).ok);

        key.approve();
        assert!(engine.run_key(&key, Trigger::Press, "test", true).ok);

        key.steps[0].settings.insert("command".into(), json!("rm -rf /"));
        assert_eq!(engine.check_key(&key), vec![Block::NeedsApproval]);
    }

    #[test]
    fn plain_credentials_and_missing_settings_stop_a_key() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![step("test.dummy/safe", json!({"password": "hunter2"}))],
            ..Key::default()
        };
        let blocks = engine.check_key(&key);
        assert!(blocks.iter().any(|b| matches!(b, Block::PlainCredential(_))));
        assert!(blocks.contains(&Block::MissingSetting("Needed thing".into())));
        assert!(!engine.run_key(&key, Trigger::Press, "test", true).ok);
    }

    #[test]
    fn each_action_waits_its_own_time_first() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![
                step("test.dummy/safe", json!({"needed": "x"})),
                Step { delay_ms: 60, ..step("test.dummy/safe", json!({"needed": "x"})) },
            ],
            ..Key::default()
        };
        let started = std::time::Instant::now();
        assert!(engine.run_key(&key, Trigger::Press, "test", true).ok);
        assert!(started.elapsed() >= Duration::from_millis(60));
    }

    #[test]
    fn a_key_with_several_actions_keeps_every_effect_and_reports_each_as_it_happens() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![
                step("core.deck/brighter", json!({"step": 10})),
                step("core.navigation/next_page", json!({})),
                step("core.deck/dimmer", json!({"step": 20})),
                step("core.navigation/next_page", json!({})),
            ],
            ..Key::default()
        };
        let mut seen = Vec::new();
        let report = engine.run_key_live(&key, Trigger::Press, "test", true, |effect| seen.push(effect));
        assert!(report.ok, "{}", report.message);
        assert_eq!(seen, vec![
            Effect::Deck(DeckControl::Brighter(10)),
            Effect::Navigate(Navigate::NextPage),
            Effect::Deck(DeckControl::Dimmer(20)),
            Effect::Navigate(Navigate::NextPage),
        ]);
        assert_eq!(report.deck.len(), 2);
        assert_eq!(report.navigate.len(), 2);
    }

    #[test]
    fn changing_an_actions_wait_ends_the_keys_approval() {
        let mut key = Key {
            steps: vec![step("test.dummy/risky", json!({"command": "ls"}))],
            ..Key::default()
        };
        key.approve();
        key.steps[0].delay_ms = 200;
        assert!(!key.is_approved());
    }

    #[test]
    fn a_key_stops_at_the_first_action_that_fails() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![
                step("core.deck/brighter", json!({})),
                step("core.navigation/go_to_page", json!({})),
                step("test.dummy/safe", json!({"needed": "x"})),
            ],
            ..Key::default()
        };
        let report = engine.run_key(&key, Trigger::Press, "test", true);
        assert!(!report.ok, "a missing setting blocks the whole key");
        assert!(!report.message.contains("ran safe"), "{}", report.message);
    }

    #[test]
    fn deck_actions_report_what_to_do() {
        let (_dir, engine) = engine();
        let run = |action: &str, settings: serde_json::Value| {
            let key = Key { steps: vec![step(action, settings)], ..Key::default() };
            let report = engine.run_key(&key, Trigger::Press, "test", true);
            assert!(report.ok, "{}", report.message);
            report.deck
        };
        assert_eq!(run("core.deck/sleep", json!({})), vec![DeckControl::Sleep]);
        assert_eq!(run("core.deck/brighter", json!({"step": 25})), vec![DeckControl::Brighter(25)]);
        assert_eq!(run("core.deck/dimmer", json!({})), vec![DeckControl::Dimmer(10)]);
        assert_eq!(run("core.deck/set_brightness", json!({"percent": 500})), vec![DeckControl::SetBrightness(100)]);
        assert_eq!(engine.spec("core.deck/sleep").unwrap().icon, SLEEP_ICON);
    }

    #[test]
    fn brightness_stays_in_range_and_dimming_cannot_blank_the_deck() {
        assert_eq!(DeckControl::Brighter(10).brightness_after(95), Some(100));
        assert_eq!(DeckControl::Dimmer(10).brightness_after(8), Some(pyroporch_sdk::LOWEST_DIM));
        assert_eq!(DeckControl::SetBrightness(200).brightness_after(50), Some(100));
        assert_eq!(DeckControl::SetBrightness(0).brightness_after(50), Some(0));
        assert_eq!(DeckControl::Sleep.brightness_after(50), None);
    }

    #[test]
    fn wait_is_off_the_list_but_old_keys_still_work() {
        let (_dir, engine) = engine();
        assert!(engine.spec("core.navigation/wait").unwrap().hidden);
        let key = Key { steps: vec![step("core.navigation/wait", json!({"seconds": 0}))], ..Key::default() };
        assert!(engine.run_key(&key, Trigger::Press, "test", true).ok);
    }

    #[test]
    fn unknown_actions_are_reported_not_run() {
        let (_dir, engine) = engine();
        let key = Key {
            steps: vec![step("nobody/nothing", json!({}))],
            ..Key::default()
        };
        assert!(matches!(engine.check_key(&key)[0], Block::UnknownAction(_)));
    }

    #[test]
    fn a_step_whose_action_is_not_installed_is_still_checked_for_credentials() {
        let (_dir, engine) = engine();
        let step = step("nobody/nothing", json!({"password": "hunter2"}));
        let blocks = engine.check_step(&step);
        assert!(
            blocks.iter().any(|block| matches!(block, Block::PlainCredential(_))),
            "{blocks:?}"
        );
    }

    #[test]
    fn things_in_the_plugins_folder_that_are_not_plugins_get_explained() {
        let (dir, engine) = engine();
        let plugins = dir.path().join("plugins");
        let write = |path: &str, text: &str| {
            let path = plugins.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        // The source of a built-in module, which is what got copied in by hand.
        write("dummy/Cargo.toml", "[package]\nname = \"pyroporch-plugin-dummy\"\n");
        write("dummy/src/lib.rs", "");
        // Source of something else.
        write("other/Cargo.toml", "[package]\nname = \"someone-elses-plugin\"\n");
        write("half/thing.wasm", "");
        write("empty/readme.txt", "");
        write("loose.wasm", "");
        write("notes.txt", "");
        write(".hidden/plugin.toml", "");
        write("_examples/api-call/Cargo.toml", "[package]\nname = \"an-example\"\n");
        std::fs::write(dir.path().join("plugins/.DS_Store"), "").unwrap();
        // Makes "dummy" count as built in, under the name the source folder would have.
        let natives: Vec<Arc<dyn Plugin>> = vec![Arc::new(Dummy)];
        let vault = Vault::with_kdf(engine.paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
        let engine = Engine::new(engine.paths.clone(), vault, natives);

        let statuses = engine.plugin_statuses();
        let note = |id: &str| statuses.iter().find(|s| s.id == id).map(|s| s.note.clone()).unwrap_or_default();
        assert!(note("other").contains("has to be built first"), "{}", note("other"));
        assert!(note("half").contains("no plugin.toml"), "{}", note("half"));
        assert!(note("empty").contains("isn't a plugin"), "{}", note("empty"));
        assert!(note("loose.wasm").contains("needs its own folder"), "{}", note("loose.wasm"));
        assert!(!statuses.iter().any(|s| s.id == "notes.txt"), "stray files are ignored");
        assert!(!statuses.iter().any(|s| s.id.starts_with('.')), "hidden things are ignored");
        assert!(!statuses.iter().any(|s| s.id.starts_with('_')), "so are folders set aside with an underscore");
        for id in ["other", "half", "empty", "loose.wasm"] {
            let status = statuses.iter().find(|s| s.id == id).unwrap();
            assert!(!status.enabled && status.sha256.is_empty(), "{id} can't be approved");
        }
    }

    #[test]
    fn the_source_of_a_built_in_module_is_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("desktop");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Cargo.toml"), "[package]\nname = \"pyroporch-desktop\"\n").unwrap();
        let note = why_not_a_plugin(&folder, &["pyroporch.desktop".to_string()]).unwrap();
        assert!(note.contains("already built into the app"), "{note}");
    }

    #[test]
    fn an_unapproved_plugin_stays_off_and_cannot_impersonate() {
        let (dir, engine) = engine();
        let plugins = dir.path().join("plugins");
        std::fs::create_dir_all(plugins.join("third")).unwrap();
        std::fs::write(plugins.join("third/plugin.toml"), "id = \"com.third\"\nwasm = \"p.wasm\"\n").unwrap();
        std::fs::write(plugins.join("third/p.wasm"), b"not really wasm").unwrap();
        std::fs::create_dir_all(plugins.join("fake")).unwrap();
        std::fs::write(plugins.join("fake/plugin.toml"), "id = \"test.dummy\"\nwasm = \"p.wasm\"\n").unwrap();
        std::fs::write(plugins.join("fake/p.wasm"), b"x").unwrap();
        std::fs::create_dir_all(plugins.join("escape")).unwrap();
        std::fs::write(plugins.join("escape/plugin.toml"), "id = \"com.esc\"\nwasm = \"../../vault.json\"\n").unwrap();
        engine.refresh_plugins();

        let statuses = engine.plugin_statuses();
        let find = |id: &str| statuses.iter().find(|s| s.id == id).unwrap();
        assert!(!find("com.third").enabled);
        assert!(find("com.third").note.contains("approval"));
        assert!(find("escape").note.contains("next to plugin.toml"));
        assert_eq!(engine.spec("test.dummy/safe").map(|s| s.id), Some("safe".into()));
        assert!(statuses.iter().any(|s| s.note.contains("already taken")));

        // Approved, but it isn't valid: stays off with a reason.
        engine.approve_plugin("com.third").unwrap();
        let status = engine.plugin_statuses().into_iter().find(|s| s.id == "com.third").unwrap();
        assert!(!status.enabled);
        assert!(status.note.contains("not a valid plugin"), "{}", status.note);
    }
}
