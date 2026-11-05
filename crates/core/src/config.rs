//! The config files. Everything the app does is described here, and edits to these files, by hand
//! or by another tool, are picked up while the app is running.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::paths::{Paths, stamp, write_private};
use crate::security::{approval_hash, sha256_hex};

pub const CONFIG_VERSION: u32 = 1;
pub const MAX_ROWS: u32 = 8;
pub const MAX_COLS: u32 = 16;
pub const DEFAULT_ACCENT: &str = "#E8743B";
/// How deep folders can go inside each other.
pub const MAX_FOLDER_DEPTH: usize = 8;
/// In a folder, this slot is the way back out.
pub const BACK_SLOT: (u32, u32) = (0, 0);
/// The themes the app has. One list, so the app, the files and the assistant agree.
pub const THEMES: [&str; 2] = ["dark", "light"];
/// The menu bar or tray icon styles.
pub const TRAY_ICONS: [&str; 3] = ["color", "white", "black"];
/// How large the window's contents may be.
pub const SCALE_RANGE: std::ops::RangeInclusive<f64> = 0.75..=1.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub active_profile: String,
    pub ui: UiSettings,
    pub security: SecuritySettings,
    pub mcp: McpSettings,
    pub hardware: HardwareSettings,
    pub updates: UpdateSettings,
    /// Closing the window leaves the app running, so the deck keeps working.
    pub run_in_background: bool,
    /// The fingerprint of each sample plugin the app has put in the plugins folder.
    pub sample_hashes: BTreeMap<String, String>,
    /// Ready-made actions defined entirely in config. They appear in the action library.
    pub presets: Vec<Preset>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            active_profile: "default".into(),
            ui: UiSettings::default(),
            security: SecuritySettings::default(),
            mcp: McpSettings::default(),
            hardware: HardwareSettings::default(),
            updates: UpdateSettings::default(),
            run_in_background: true,
            sample_hashes: BTreeMap::new(),
            presets: Vec::new(),
        }
    }
}

impl Settings {
    /// What is wrong with these settings. They are still used, so a hand-edited file keeps
    /// working, but the person is told about it.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !THEMES.contains(&self.ui.theme.as_str()) {
            problems.push(format!("there is no theme called {}", self.ui.theme));
        }
        if !TRAY_ICONS.contains(&self.ui.tray_icon.as_str()) {
            problems.push(format!("there is no menu bar icon called {}", self.ui.tray_icon));
        }
        if !SCALE_RANGE.contains(&f64::from(self.ui.scale)) {
            problems.push(format!("the size should be from {} to {}", SCALE_RANGE.start(), SCALE_RANGE.end()));
        }
        if self.hardware.brightness > 100 {
            problems.push("the deck's brightness should be from 0 to 100".into());
        }
        problems
    }
}

/// Looking for new versions. Off until the person asks, as it means calling out to GitHub.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Looks on the releases page every 6 hours. Installing one always waits for the person.
    pub check_automatically: bool,
    /// When it last looked, in seconds since 1970.
    pub last_checked: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: String,
    pub accent: String,
    pub scale: f32,
    /// The menu bar or tray icon: `color`, `white` or `black`.
    pub tray_icon: String,
    /// The groups in the action list that are folded away.
    pub collapsed_groups: Vec<String>,
    /// The colours kept for choosing again, on any key.
    pub favorite_colors: Vec<String>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            accent: DEFAULT_ACCENT.into(),
            scale: 1.0,
            tray_icon: "color".into(),
            collapsed_groups: Vec::new(),
            favorite_colors: ["#2F6FDB", "#0F8B8D", "#2E8B57", "#7A4FD0", "#A83279", "#B4532A", "#C0392B", "#B8860B", "#3A4252", "#14171C"].map(String::from).to_vec(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecuritySettings {
    /// Lock the vault after this many idle minutes. Zero means never.
    pub auto_lock_minutes: u32,
    /// Let keys pressed by an AI assistant use vault secrets, when this device remembers the key.
    pub mcp_may_use_secrets: bool,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            auto_lock_minutes: 15,
            mcp_may_use_secrets: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    pub enabled: bool,
    /// An assistant can look but not change anything.
    pub read_only: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            read_only: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HardwareSettings {
    pub enabled: bool,
    pub brightness: u8,
}

impl Default for HardwareSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            brightness: 70,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub icon: String,
    pub color: String,
    pub action: String,
    pub settings: Map<String, Value>,
}

/// One step of what a key does. A key with several steps is a multi-action.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// `plugin_id/action_id`
    pub action: String,
    pub settings: Map<String, Value>,
    /// Wait this long before the step.
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Key {
    pub row: u32,
    pub col: u32,
    pub title: String,
    /// Which part of the key its title sits against: `top`, `middle` or `bottom`.
    pub title_position: String,
    /// One character, or a path to a PNG or JPEG.
    pub icon: String,
    /// A stored picture filling the key, by its file name in the images folder.
    pub background: String,
    pub color: String,
    /// The icon's colour, when the icon is a character. The same as `text_color` if empty.
    pub icon_color: String,
    pub text_color: String,
    /// Pressing flips between two looks.
    pub toggle: bool,
    pub alt_title: String,
    pub alt_color: String,
    pub steps: Vec<Step>,
    /// Fingerprint of the steps the person approved. Needed for actions that touch the computer.
    pub approved: String,
    /// When set, pressing this key opens its own screen of keys instead of doing anything.
    pub folder: Option<Box<Page>>,
    /// Read from files saved by the short-lived macro version, then moved onto the steps.
    #[serde(rename = "macro_delay_ms", skip_serializing)]
    pub old_macro_delay_ms: u64,
}

impl Key {
    /// Marks the key's current steps as approved by the person.
    pub fn approve(&mut self) {
        self.approved = approval_hash(&self.steps);
    }
    pub fn is_approved(&self) -> bool {
        self.approved == approval_hash(&self.steps)
    }

    /// Gives each later step the wait an old macro had for all of them. A key the person had
    /// approved stays approved: this is the same timing, stored differently.
    fn absorb_old_macro_delay(&mut self) {
        let delay = std::mem::take(&mut self.old_macro_delay_ms);
        if delay == 0 {
            return;
        }
        let old_fingerprint = sha256_hex(format!("{}\0{delay}", approval_hash(&self.steps)).as_bytes());
        let was_approved = self.approved == old_fingerprint;
        for step in self.steps.iter_mut().skip(1).filter(|step| step.delay_ms == 0) {
            step.delay_ms = delay;
        }
        if was_approved {
            self.approve();
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Page {
    pub name: String,
    pub keys: Vec<Key>,
}

impl Page {
    pub fn key_at(&self, row: u32, col: u32) -> Option<&Key> {
        self.keys.iter().find(|k| k.row == row && k.col == col)
    }
    pub fn key_at_mut(&mut self, row: u32, col: u32) -> Option<&mut Key> {
        self.keys.iter_mut().find(|k| k.row == row && k.col == col)
    }
    pub fn remove_at(&mut self, row: u32, col: u32) -> Option<Key> {
        let index = self.keys.iter().position(|k| k.row == row && k.col == col)?;
        Some(self.keys.remove(index))
    }
    pub fn put(&mut self, key: Key) {
        self.remove_at(key.row, key.col);
        self.keys.push(key);
    }

    /// The screen reached by opening each folder in `path`, starting from this one.
    pub fn descend(&self, path: &[(u32, u32)]) -> Option<&Page> {
        let mut page = self;
        for (row, col) in path {
            page = page.key_at(*row, *col)?.folder.as_deref()?;
        }
        Some(page)
    }

    pub fn descend_mut(&mut self, path: &[(u32, u32)]) -> Option<&mut Page> {
        let mut page = self;
        for (row, col) in path {
            page = page.key_at_mut(*row, *col)?.folder.as_deref_mut()?;
        }
        Some(page)
    }

    /// The first empty slot on a folder's screen, leaving its back key's slot alone.
    pub fn free_folder_slot(&self, rows: u32, cols: u32) -> Option<(u32, u32)> {
        (0..rows)
            .flat_map(|row| (0..cols).map(move |col| (row, col)))
            .find(|slot| *slot != BACK_SLOT && self.key_at(slot.0, slot.1).is_none())
    }

    /// Visits every key on this screen and inside its folders.
    pub fn for_each_key_mut(&mut self, visit: &mut impl FnMut(&mut Key)) {
        for key in &mut self.keys {
            visit(key);
            if let Some(folder) = &mut key.folder {
                folder.for_each_key_mut(visit);
            }
        }
    }

    pub fn for_each_key(&self, visit: &mut impl FnMut(&Key)) {
        for key in &self.keys {
            visit(key);
            if let Some(folder) = &key.folder {
                folder.for_each_key(visit);
            }
        }
    }

    /// How many folders deep this screen goes.
    pub fn depth(&self) -> usize {
        self.keys
            .iter()
            .filter_map(|key| key.folder.as_ref())
            .map(|folder| folder.depth() + 1)
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub version: u32,
    pub name: String,
    pub rows: u32,
    pub cols: u32,
    pub pages: Vec<Page>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            name: "New Profile".into(),
            rows: 3,
            cols: 5,
            pages: vec![Page {
                name: "Main".into(),
                keys: Vec::new(),
            }],
        }
    }
}

impl Profile {
    /// Brings fields from older versions up to date.
    pub fn upgrade_old_fields(&mut self) {
        for page in &mut self.pages {
            page.for_each_key_mut(&mut Key::absorb_old_macro_delay);
        }
    }

    /// Keeps the numbers usable and every key on the grid. Keys are kept, not thrown away, if the
    /// grid has simply been made smaller: they come back when it grows.
    pub fn validate(&self) -> Result<(), String> {
        if self.version > CONFIG_VERSION {
            return Err("made by a newer version of Pyro Porch".into());
        }
        if !(1..=MAX_ROWS).contains(&self.rows) || !(1..=MAX_COLS).contains(&self.cols) {
            return Err(format!("the grid must be 1 to {MAX_ROWS} rows and 1 to {MAX_COLS} columns"));
        }
        if self.pages.is_empty() {
            return Err("a profile needs at least one page".into());
        }
        if self.pages.iter().any(|page| page.depth() > MAX_FOLDER_DEPTH) {
            return Err(format!("folders can only go {MAX_FOLDER_DEPTH} deep"));
        }
        Ok(())
    }

    /// The sample deck shown on first start. Written by the app itself, so its keys are approved.
    /// `with_desktop` says whether the Desktop actions are in this build: a key pointing at an
    /// action that isn't there could never run.
    pub fn sample(with_desktop: bool) -> Self {
        fn key(row: u32, col: u32, title: &str, icon: &str, color: &str, steps: Vec<Step>) -> Key {
            let mut key = Key {
                row,
                col,
                title: title.into(),
                icon: icon.into(),
                color: color.into(),
                steps,
                ..Key::default()
            };
            key.approve();
            key
        }
        fn step(action: &str, settings: Value) -> Step {
            Step {
                action: action.into(),
                settings: settings.as_object().cloned().unwrap_or_default(),
                delay_ms: 0,
            }
        }
        use serde_json::json;
        let media = |key_name: &str| vec![step("pyroporch.desktop/media", json!({"key": key_name}))];
        let mut main = vec![key(0, 1, "API Status", "🌐", "#0F8B8D", vec![step("pyroporch.api-call/request", json!({
            "url": "https://api.github.com/zen", "method": "GET", "auth": "none", "timeout_seconds": 15
        }))])];
        if with_desktop {
            main.push(key(0, 0, "Website", "🔗", "#2F6FDB", vec![step("pyroporch.desktop/open_url", json!({"url": "https://example.com"}))]));
            main.push(key(0, 2, "Play", "▶", "#7A4FD0", media("play/pause")));
            main.push(key(0, 3, "Quieter", "🔉", "#7A4FD0", media("volume down")));
            main.push(key(0, 4, "Louder", "🔊", "#7A4FD0", media("volume up")));
            main.push(key(1, 0, "New Tab", "⌨", "#B4532A", vec![step("pyroporch.desktop/hotkey", json!({"keys": "cmd+t"}))]));
            main.push(key(1, 1, "Sign-off", "✏", "#B4532A", vec![step("pyroporch.desktop/type_text", json!({"text": "Thanks, talk soon!"}))]));
            main.push(key(1, 2, "Work Mode", "🚀", "#A83279", vec![
                step("pyroporch.desktop/open_url", json!({"url": "https://example.com/calendar"})),
                Step { delay_ms: 500, ..step("pyroporch.desktop/open_url", json!({"url": "https://example.com/mail"})) },
            ]));
            main.push(Key {
                toggle: true,
                alt_title: "Muted".into(),
                alt_color: "#C0392B".into(),
                ..key(1, 3, "Sound On", "🔈", "#2E8B57", media("mute"))
            });
            main.push(Key {
                folder: Some(Box::new(Page {
                    name: "Media".into(),
                    keys: vec![
                        key(0, 1, "Previous", "⏮", "#7A4FD0", media("previous")),
                        key(0, 2, "Next", "⏭", "#7A4FD0", media("next")),
                        key(0, 3, "Mute", "🔇", "#7A4FD0", media("mute")),
                    ],
                })),
                ..key(1, 4, "Media", "📁", "#3A4252", Vec::new())
            });
        }
        main.push(key(2, 4, "More", "➡", "#3A4252", vec![step("core.navigation/next_page", json!({}))]));
        let second = vec![
            key(0, 0, "Sleep", crate::engine::SLEEP_ICON, "#1C1F26", vec![step("core.deck/sleep", json!({}))]),
            key(0, 1, "Dimmer", "🔅", "#3A4252", vec![step("core.deck/dimmer", json!({"step": 10}))]),
            key(0, 2, "Brighter", "🔆", "#3A4252", vec![step("core.deck/brighter", json!({"step": 10}))]),
            key(0, 3, "Saver", "🌈", "#3A4252", vec![step("pyroporch.screensaver/toggle", json!({
                "animation": "Rainbow: side to side", "start_now": false, "wait_minutes": 5, "speed": "normal"
            }))]),
            key(2, 0, "Back", "⬅", "#3A4252", vec![step("core.navigation/previous_page", json!({}))]),
        ];
        Self {
            name: "Default".into(),
            pages: vec![
                Page { name: "Main".into(), keys: main },
                Page { name: "Second".into(), keys: second },
            ],
            ..Self::default()
        }
    }
}

/// Turns a display name into a safe file name.
pub fn profile_id(name: &str) -> String {
    let id: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if id.is_empty() { "profile".into() } else { id }
}

/// All of the config, loaded. Keeps the last good copy if a file can't be read, and says why.
pub struct Workspace {
    pub paths: Paths,
    pub settings: Settings,
    pub profiles: BTreeMap<String, Profile>,
    /// Problems found the last time the files were read.
    pub errors: Vec<String>,
    settings_stamp: Option<std::time::SystemTime>,
    /// The settings as they were last read, so a save can tell what we changed from what another
    /// program changed.
    settings_loaded: Option<Settings>,
    /// Profiles with work that isn't saved yet, and the autosave file holding it.
    unsaved: BTreeMap<String, PathBuf>,
    /// `config.toml` has been read successfully at least once, so `settings` came from it.
    settings_read: bool,
    /// `config.toml` is there but can't be used right now, so it mustn't be saved over.
    settings_broken: bool,
    grants_stamp: Option<std::time::SystemTime>,
    profile_stamps: BTreeMap<String, Option<std::time::SystemTime>>,
    /// The profiles as they were last read from their files, so a save can tell what we
    /// changed from what another program changed.
    loaded: BTreeMap<String, Profile>,
}

pub(crate) fn to_toml<T: Serialize>(value: &T) -> Result<String, String> {
    // TOML has no null, so empties are dropped first.
    let mut json = serde_json::to_value(value).map_err(|e| e.to_string())?;
    drop_nulls(&mut json);
    toml::to_string_pretty(&json).map_err(|e| e.to_string())
}

fn drop_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            map.values_mut().for_each(drop_nulls);
        }
        Value::Array(items) => {
            items.retain(|v| !v.is_null());
            items.iter_mut().for_each(drop_nulls);
        }
        _ => {}
    }
}

const SETTINGS_HEADER: &str = "# Pyro Porch settings. Edits here are picked up while the app is running.\n# See docs/CONFIG.md for every option.\n\n";
const PROFILE_HEADER: &str = "# A Pyro Porch profile: a grid of keys, across one or more pages.\n# Secrets never go here; refer to the vault with {{secret:name}}.\n\n";

/// Reads a profile from its file, if it can be read.
/// `None` means there's no file. A file that can't be used is an error, so it isn't saved over.
fn read_profile(path: &Path) -> Result<Option<Profile>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut profile: Profile = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    profile.validate()?;
    profile.upgrade_old_fields();
    Ok(Some(profile))
}

/// An autosave name for a profile with no file yet, from the local date and time.
fn free_dated_autosave(dir: &Path) -> PathBuf {
    let date = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
    let mut path = dir.join(format!("{date}_autosave.toml"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{date}-{n}_autosave.toml"));
        n += 1;
    }
    path
}

/// Puts our changes to the settings together with anyone else's, setting by setting.
fn merge_settings(before: Option<&Settings>, ours: &Settings, disk: &Settings) -> Result<Settings, String> {
    let Some(before) = before else {
        return Ok(ours.clone());
    };
    let value = |settings: &Settings| serde_json::to_value(settings).map_err(|e| e.to_string());
    let merged = merge_values(&value(before)?, &value(ours)?, &value(disk)?);
    serde_json::from_value(merged).map_err(|e| e.to_string())
}

/// What we didn't change stays as the file has it. If both sides changed something, tables are
/// looked into, a list of named things is put together by name, and otherwise ours stands.
fn merge_values(before: &serde_json::Value, ours: &serde_json::Value, disk: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    if ours == before {
        return disk.clone();
    }
    if disk == before {
        return ours.clone();
    }
    match (before, ours, disk) {
        (Value::Object(before), Value::Object(ours), Value::Object(disk)) => {
            let mut merged = disk.clone();
            for (key, our_value) in ours {
                let inner = match (before.get(key), disk.get(key)) {
                    (Some(before_value), Some(disk_value)) => merge_values(before_value, our_value, disk_value),
                    _ => our_value.clone(),
                };
                merged.insert(key.clone(), inner);
            }
            merged.retain(|key, _| ours.contains_key(key) || !before.contains_key(key));
            Value::Object(merged)
        }
        (Value::Array(before), Value::Array(ours), Value::Array(disk)) if [before, ours, disk].iter().all(|list| all_named(list)) => {
            let find = |list: &[Value], name: &Value| list.iter().find(|item| item["name"] == *name).cloned();
            let mut merged: Vec<Value> = Vec::new();
            for item in disk {
                let name = &item["name"];
                match (find(ours, name), find(before, name)) {
                    (None, Some(_)) => {}
                    (Some(our_item), Some(before_item)) if our_item != before_item => merged.push(our_item),
                    (Some(our_item), None) => merged.push(our_item),
                    _ => merged.push(item.clone()),
                }
            }
            for item in ours {
                if find(disk, &item["name"]).is_none() && find(before, &item["name"]).is_none_or(|old| old != *item) {
                    merged.push(item.clone());
                }
            }
            Value::Array(merged)
        }
        _ => ours.clone(),
    }
}

fn all_named(list: &[serde_json::Value]) -> bool {
    list.iter().all(|item| item["name"].is_string())
}

/// Puts our changes together with anyone else's, so a save doesn't throw away a change another
/// program (the MCP server) made to the same file while we were editing.
fn merge_profiles(before: Option<&Profile>, ours: &Profile, disk: &Profile) -> Profile {
    // A change to the shape of the profile is ours alone: the screens can't be lined up.
    if before.is_some_and(|before| before.name != ours.name || before.rows != ours.rows || before.cols != ours.cols) {
        return ours.clone();
    }
    let mut merged = disk.clone();
    for (index, our_page) in ours.pages.iter().enumerate() {
        let before_page = before.and_then(|before| before.pages.get(index));
        let disk_page = disk.pages.get(index).cloned().unwrap_or_else(|| our_page.clone());
        let merged_page = merge_pages(before_page, our_page, &disk_page);
        if index < merged.pages.len() {
            merged.pages[index] = merged_page;
        } else {
            merged.pages.push(merged_page);
        }
    }
    // A page we took away stays away.
    if let Some(before) = before
        && before.pages.len() > ours.pages.len()
    {
        merged.pages.truncate(ours.pages.len());
    }
    merged
}

/// Puts the changes to one screen together, key by key.
fn merge_pages(before: Option<&Page>, ours: &Page, disk: &Page) -> Page {
    let mut keys: Vec<Key> = Vec::with_capacity(disk.keys.len() + ours.keys.len());
    for disk_key in &disk.keys {
        let our_key = ours.key_at(disk_key.row, disk_key.col);
        let before_key = before.and_then(|before| before.key_at(disk_key.row, disk_key.col));
        let chosen = match (our_key, before_key) {
            // We took it away.
            (None, Some(_)) => None,
            // We never had it, so it is theirs.
            (None, None) => Some(disk_key),
            // We didn't change it, so theirs stands.
            (Some(ours), Some(before)) if ours == before => Some(disk_key),
            // We changed it, so ours stands.
            (Some(ours), _) => Some(ours),
        };
        keys.extend(chosen.cloned());
    }
    // Keys only we have. One we had all along and didn't change has been removed or moved by
    // someone else, so it stays away.
    for our_key in &ours.keys {
        let unchanged = before.and_then(|before| before.key_at(our_key.row, our_key.col)) == Some(our_key);
        if disk.key_at(our_key.row, our_key.col).is_none() && !unchanged {
            keys.push(our_key.clone());
        }
    }
    let name = match before {
        Some(before) if before.name == ours.name => disk.name.clone(),
        _ => ours.name.clone(),
    };
    Page { name, keys }
}

impl Workspace {
    /// Reads everything, creating the starter files on first run.
    pub fn load(paths: Paths) -> Result<Self, String> {
        Self::load_with(paths, true)
    }

    /// The same, but the starter deck leaves out the Desktop actions when this build doesn't
    /// have them: a key pointing at an action that isn't there could never run.
    pub fn load_with(paths: Paths, with_desktop: bool) -> Result<Self, String> {
        paths.ensure().map_err(|e| format!("couldn't create {}: {e}", paths.root.display()))?;
        let mut ws = Self {
            paths,
            settings: Settings::default(),
            profiles: BTreeMap::new(),
            errors: Vec::new(),
            settings_stamp: None,
            settings_loaded: None,
            unsaved: BTreeMap::new(),
            settings_read: false,
            settings_broken: false,
            grants_stamp: None,
            profile_stamps: BTreeMap::new(),
            loaded: BTreeMap::new(),
        };
        if !ws.paths.config().exists() {
            ws.save_settings()?;
        }
        ws.reload_settings();
        let empty = std::fs::read_dir(ws.paths.profiles_dir()).map_or(true, |mut d| d.next().is_none());
        if empty {
            ws.profiles.insert("default".into(), Profile::sample(with_desktop));
            ws.save_profile("default")?;
        }
        ws.check_reload();
        Ok(ws)
    }

    fn reload_settings(&mut self) -> bool {
        let path = self.paths.config();
        let now = stamp(&path);
        if now == self.settings_stamp {
            return false;
        }
        self.settings_stamp = now;
        self.errors.retain(|e| !e.starts_with("config.toml"));
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
            toml::from_str::<Settings>(&text).map_err(|e| e.message().to_string())
        }) {
            Ok(settings) if settings.version > CONFIG_VERSION => {
                self.refuse_settings_file("made by a newer version of Pyro Porch");
                false
            }
            Ok(settings) => {
                // A hand-edited file may hold values the app doesn't have. They are still
                // used, but the person is told.
                self.errors.extend(settings.problems().into_iter().map(|problem| format!("config.toml: {problem}")));
                self.settings_loaded = Some(settings.clone());
                self.settings = settings;
                self.settings_read = true;
                self.settings_broken = false;
                true
            }
            Err(e) => {
                self.refuse_settings_file(&e);
                false
            }
        }
    }

    /// The file can't be used. It's left alone for the person to fix. If it has never been read,
    /// what it says about assistant access is unknown, so the safe answer is used until it is.
    fn refuse_settings_file(&mut self, problem: &str) {
        self.settings_broken = self.paths.config().exists();
        let kept = if self.settings_read {
            "Keeping the last good settings"
        } else {
            self.settings.mcp.enabled = false;
            self.settings.mcp.read_only = true;
            "Using the default settings with assistant access off"
        };
        self.errors.push(format!("config.toml: {problem}. {kept}, and not saving over the file until it's fixed."));
    }

    /// `config.toml` is there but can't be used, so changes to the settings can't be saved yet.
    pub fn settings_file_broken(&self) -> bool {
        self.settings_broken
    }

    /// Says what's wrong with `grants.toml`, whenever the file changes.
    fn check_grants_file(&mut self) {
        let now = stamp(&self.paths.grants());
        if now == self.grants_stamp {
            return;
        }
        self.grants_stamp = now;
        self.errors.retain(|e| !e.starts_with("grants.toml"));
        if let Err(problem) = crate::policy::Grants::try_load(&self.paths) {
            self.errors.push(problem);
        }
    }

    /// Picks up edits made outside the app. Returns true if anything changed.
    pub fn check_reload(&mut self) -> bool {
        let mut changed = self.reload_settings();
        self.check_grants_file();
        let mut seen = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.paths.profiles_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                // Only plain files: reading a pipe could wait forever, and this runs at every start.
                if path.extension().and_then(|e| e.to_str()) != Some("toml") || !path.is_file() {
                    continue;
                }
                let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                    continue;
                };
                seen.push(id.clone());
                let now = stamp(&path);
                if self.profile_stamps.get(&id) == Some(&now) {
                    continue;
                }
                self.profile_stamps.insert(id.clone(), now);
                let label = format!("{id}.toml");
                self.errors.retain(|e| !e.starts_with(&label));
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
                    let mut profile = toml::from_str::<Profile>(&text).map_err(|e| e.message().to_string())?;
                    profile.validate()?;
                    profile.upgrade_old_fields();
                    Ok(profile)
                }) {
                    // The person is partway through changing this one. The two come together
                    // when they save it.
                    Ok(_) if self.unsaved.contains_key(&id) => {}
                    Ok(profile) => {
                        self.profiles.insert(id.clone(), profile.clone());
                        self.loaded.insert(id, profile);
                        changed = true;
                    }
                    Err(e) => self.errors.push(format!("{label}: {e}. Keeping the last good copy.")),
                }
            }
        }
        let gone: Vec<String> = self.profiles.keys().filter(|id| !seen.contains(id) && !self.unsaved.contains_key(*id)).cloned().collect();
        for id in gone {
            self.profiles.remove(&id);
            self.profile_stamps.remove(&id);
            changed = true;
        }
        if !self.profiles.contains_key(&self.settings.active_profile)
            && let Some(first) = self.profiles.keys().next()
        {
            self.settings.active_profile = first.clone();
        }
        changed
    }

    pub fn save_settings(&mut self) -> Result<(), String> {
        if self.settings_broken {
            return Err("config.toml has a mistake, so it wasn't saved over. Fix it first".into());
        }
        // Another program may have changed this file since we last read it, so our changes are
        // put together with theirs instead of overwriting them.
        let merged = match self.read_settings_for_merge()? {
            Some(disk) => merge_settings(self.settings_loaded.as_ref(), &self.settings, &disk)?,
            None => self.settings.clone(),
        };
        let text = format!("{SETTINGS_HEADER}{}", to_toml(&merged)?);
        write_private(&self.paths.config(), text.as_bytes()).map_err(|e| e.to_string())?;
        self.settings_stamp = stamp(&self.paths.config());
        self.settings_loaded = Some(merged.clone());
        self.settings = merged;
        self.settings_read = true;
        Ok(())
    }

    fn read_settings_for_merge(&self) -> Result<Option<Settings>, String> {
        let text = match std::fs::read_to_string(self.paths.config()) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("couldn't read config.toml, so it wasn't saved over: {e}")),
        };
        let disk: Settings = toml::from_str(&text).map_err(|e| format!("config.toml has a mistake ({}), so it wasn't saved over. Fix it first", e.message()))?;
        if disk.version > CONFIG_VERSION {
            return Err("config.toml was made by a newer version of Pyro Porch, so it wasn't saved over".into());
        }
        Ok(Some(disk))
    }

    /// Keeps the person's work without touching the profile's own file. The file is named after
    /// the profile's once it has one, and after the date until then.
    pub fn autosave_profile(&mut self, id: &str) -> Result<(), String> {
        let profile = self.profiles.get(id).ok_or("no such profile")?;
        let path = match self.unsaved.get(id) {
            Some(path) => path.clone(),
            None if self.paths.profile(id).exists() => self.paths.autosave_dir().join(format!("{id}_autosave.toml")),
            None => free_dated_autosave(&self.paths.autosave_dir()),
        };
        let text = format!("{PROFILE_HEADER}{}", to_toml(profile)?);
        write_private(&path, text.as_bytes()).map_err(|e| e.to_string())?;
        self.unsaved.insert(id.to_string(), path);
        Ok(())
    }

    /// Whether this profile has work that isn't in its own file yet.
    pub fn has_unsaved(&self, id: &str) -> bool {
        self.unsaved.contains_key(id)
    }

    /// Picks up work left in autosave files after the app closed, as unsaved work. Returns the
    /// profiles that had some. A file that can't be read is left there and reported.
    pub fn recover_autosaves(&mut self) -> Vec<String> {
        let mut recovered = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.paths.autosave_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("toml"))
            .collect();
        files.sort();
        for path in files {
            let label = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let profile = match read_profile(&path) {
                Ok(Some(profile)) => profile,
                Ok(None) => continue,
                Err(e) => {
                    self.errors.push(format!("{label}: {e}. It was left in the autosave folder."));
                    continue;
                }
            };
            let named = label.strip_suffix("_autosave.toml").filter(|id| self.profiles.contains_key(*id)).map(str::to_string);
            let id = named.unwrap_or_else(|| self.free_profile_id(&profile.name));
            if self.profiles.get(&id) == Some(&profile) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            self.profiles.insert(id.clone(), profile);
            self.unsaved.insert(id.clone(), path);
            recovered.push(id);
        }
        recovered
    }

    /// A file name for a new profile that nothing else is using.
    fn free_profile_id(&self, name: &str) -> String {
        let base = profile_id(name);
        let mut id = base.clone();
        let mut n = 2;
        // A file that couldn't be read isn't in the list, but its name is still taken.
        while self.profiles.contains_key(&id) || self.paths.profile(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Adds a profile the person hasn't saved yet: it only exists in an autosave file.
    pub fn add_unsaved_profile(&mut self, profile: Profile) -> Result<String, String> {
        let id = self.free_profile_id(&profile.name);
        self.profiles.insert(id.clone(), profile);
        self.autosave_profile(&id)?;
        Ok(id)
    }

    pub fn save_profile(&mut self, id: &str) -> Result<(), String> {
        let profile = self.profiles.get(id).ok_or("no such profile")?;
        let path = self.paths.profile(id);
        // Another program may have changed this file since we last read it, so our changes are
        // put together with theirs instead of overwriting them.
        let on_disk = read_profile(&path).map_err(|e| format!("{id}.toml has a mistake ({e}), so it wasn't saved over. Fix it first"))?;
        let merged = match on_disk {
            Some(disk) => merge_profiles(self.loaded.get(id), profile, &disk),
            None => profile.clone(),
        };
        merged.validate()?;
        let text = format!("{PROFILE_HEADER}{}", to_toml(&merged)?);
        write_private(&path, text.as_bytes()).map_err(|e| e.to_string())?;
        self.profiles.insert(id.to_string(), merged.clone());
        self.loaded.insert(id.to_string(), merged);
        self.profile_stamps.insert(id.to_string(), stamp(&path));
        // It's all in its own file now, so the autosave has done its job.
        if let Some(autosave) = self.unsaved.remove(id) {
            let _ = std::fs::remove_file(autosave);
        }
        Ok(())
    }

    /// Adds a profile, finding a free file name. Returns its id.
    pub fn add_profile(&mut self, profile: Profile) -> Result<String, String> {
        let id = self.free_profile_id(&profile.name);
        self.profiles.insert(id.clone(), profile);
        self.save_profile(&id)?;
        Ok(id)
    }

    pub fn delete_profile(&mut self, id: &str) -> Result<(), String> {
        if self.profiles.len() <= 1 {
            return Err("keep at least one profile".into());
        }
        self.profiles.remove(id);
        self.profile_stamps.remove(id);
        self.loaded.remove(id);
        if let Some(autosave) = self.unsaved.remove(id) {
            let _ = std::fs::remove_file(autosave);
        }
        // A profile that was never saved has no file.
        match std::fs::remove_file(self.paths.profile(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
        if self.settings.active_profile == id
            && let Some(first) = self.profiles.keys().next().cloned()
        {
            self.settings.active_profile = first;
            self.save_settings()?;
        }
        Ok(())
    }

    pub fn active(&self) -> Option<&Profile> {
        self.profiles.get(&self.settings.active_profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::load(Paths::at(dir.path())).unwrap();
        (dir, ws)
    }

    /// Waits long enough for the file's changed time to differ on any file system.
    fn write_later(path: &std::path::Path, text: &str) {
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn first_run_creates_a_sample_that_survives_a_round_trip() {
        let (dir, ws) = workspace();
        let sample = ws.profiles.get("default").unwrap();
        assert!(sample.pages[0].key_at(0, 0).is_some());
        assert!(sample.pages[0].keys.iter().all(Key::is_approved));
        let again = Workspace::load(Paths::at(dir.path())).unwrap();
        assert_eq!(again.profiles.get("default"), Some(sample));
    }

    #[test]
    fn hand_edits_are_picked_up_live() {
        let (dir, mut ws) = workspace();
        let path = dir.path().join("profiles/studio.toml");
        write_later(&path, "name = \"Studio\"\nrows = 2\ncols = 3\n[[pages]]\nname = \"One\"\n");
        assert!(ws.check_reload());
        assert_eq!(ws.profiles["studio"].cols, 3);

        write_later(&dir.path().join("config.toml"), "active_profile = \"studio\"\n[ui]\nscale = 1.5\n");
        assert!(ws.check_reload());
        assert_eq!(ws.settings.ui.scale, 1.5);
        assert_eq!(ws.active().unwrap().name, "Studio");
    }

    #[test]
    fn a_broken_edit_keeps_the_last_good_copy() {
        let (dir, mut ws) = workspace();
        write_later(&dir.path().join("profiles/default.toml"), "rows = [not valid");
        ws.check_reload();
        assert!(ws.profiles.contains_key("default"));
        assert_eq!(ws.errors.len(), 1);
    }

    #[test]
    fn a_config_file_that_cant_be_read_at_the_start_turns_assistant_access_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[mcp]\nenabled = false\n[security]\nauto_lock_minutes = \"ten\"\n";
        std::fs::write(&path, text).unwrap();
        let mut ws = Workspace::load(Paths::at(dir.path())).unwrap();
        assert!(!ws.settings.mcp.enabled);
        assert!(ws.settings.mcp.read_only);
        assert!(ws.errors[0].contains("assistant access off"), "{:?}", ws.errors);

        // Saving is refused, so the person's choice isn't replaced with the defaults.
        assert!(ws.save_settings().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        // Once it's fixed, everything goes back to normal.
        write_later(&path, "[mcp]\nenabled = true\n");
        assert!(ws.check_reload());
        assert!(ws.settings.mcp.enabled);
        assert!(ws.errors.is_empty(), "{:?}", ws.errors);
        ws.save_settings().unwrap();
    }

    #[test]
    fn a_config_file_that_breaks_later_keeps_the_last_good_settings_and_the_file() {
        let (dir, mut ws) = workspace();
        let path = dir.path().join("config.toml");
        write_later(&path, "[ui]\nscale = 1.5\n");
        ws.check_reload();
        write_later(&path, "version = 999\n");
        ws.check_reload();
        assert_eq!(ws.settings.ui.scale, 1.5);
        assert!(ws.settings.mcp.enabled, "the person's settings are still known");
        assert!(ws.save_settings().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = 999\n");
    }

    #[test]
    fn a_save_doesnt_bring_back_a_key_someone_else_removed_or_moved() {
        let (dir, mut ws) = workspace();
        let keys = ws.profiles["default"].pages[0].keys.len();
        assert!(keys >= 2);
        let (row, col) = (ws.profiles["default"].pages[0].keys[0].row, ws.profiles["default"].pages[0].keys[0].col);

        let mut other = Workspace::load(Paths::at(dir.path())).unwrap();
        other.profiles.get_mut("default").unwrap().pages[0].remove_at(row, col);
        other.save_profile("default").unwrap();

        // Our change to another key goes in, and the removed key doesn't come back.
        ws.profiles.get_mut("default").unwrap().pages[0].keys[1].title = "Ours".into();
        ws.save_profile("default").unwrap();
        let page = &ws.profiles["default"].pages[0];
        assert_eq!(page.keys.len(), keys - 1);
        assert!(page.key_at(row, col).is_none());
        assert!(page.keys.iter().any(|key| key.title == "Ours"));
    }

    #[test]
    fn saving_settings_keeps_what_another_program_changed() {
        let (dir, mut ws) = workspace();
        let path = dir.path().join("config.toml");
        ws.settings.ui.scale = 1.5;
        // The person turns assistant access off and an assistant adds a preset, on disk.
        let mut other = Workspace::load(Paths::at(dir.path())).unwrap();
        other.settings.mcp.enabled = false;
        other.settings.presets.push(Preset { name: "Theirs".into(), ..Preset::default() });
        other.save_settings().unwrap();

        ws.settings.presets.push(Preset { name: "Ours".into(), ..Preset::default() });
        ws.save_settings().unwrap();
        for settings in [&ws.settings, &toml::from_str::<Settings>(&std::fs::read_to_string(&path).unwrap()).unwrap()] {
            assert!(!settings.mcp.enabled, "their change stands");
            assert_eq!(settings.ui.scale, 1.5, "ours stands too");
            let names: Vec<&str> = settings.presets.iter().map(|preset| preset.name.as_str()).collect();
            assert_eq!(names, vec!["Theirs", "Ours"]);
        }
    }

    #[test]
    fn autosaves_never_touch_the_profiles_own_file() {
        let (dir, mut ws) = workspace();
        let main = dir.path().join("profiles/default.toml");
        let before = std::fs::read_to_string(&main).unwrap();
        ws.profiles.get_mut("default").unwrap().name = "Changed".into();
        ws.autosave_profile("default").unwrap();
        assert!(ws.has_unsaved("default"));
        assert_eq!(std::fs::read_to_string(&main).unwrap(), before);
        let autosave = dir.path().join("autosave/default_autosave.toml");
        assert!(std::fs::read_to_string(&autosave).unwrap().contains("Changed"));

        // The app doesn't take it for a profile, and an outside edit doesn't undo the work.
        write_later(&main, &before.replace("rows = 3", "rows = 4"));
        ws.check_reload();
        assert_eq!(ws.profiles.len(), 1);
        assert_eq!(ws.profiles["default"].name, "Changed");

        // Saving puts it in the file and clears the autosave.
        ws.save_profile("default").unwrap();
        assert!(!ws.has_unsaved("default"));
        assert!(!autosave.exists());
        assert!(std::fs::read_to_string(&main).unwrap().contains("Changed"));
    }

    #[test]
    fn a_profile_that_was_never_saved_autosaves_under_the_date_until_it_is() {
        let (dir, mut ws) = workspace();
        let id = ws.add_unsaved_profile(Profile { name: "Studio".into(), ..Profile::default() }).unwrap();
        assert_eq!(id, "studio");
        assert!(!dir.path().join("profiles/studio.toml").exists());
        let files: Vec<String> = std::fs::read_dir(dir.path().join("autosave")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(files.len(), 1);
        let today = chrono::Local::now().format("%Y-%m-%d_").to_string();
        assert!(files[0].starts_with(&today) && files[0].ends_with("_autosave.toml"), "{files:?}");

        // It survives a look at the folder, and the same file keeps being used.
        ws.check_reload();
        assert!(ws.profiles.contains_key("studio"));
        ws.autosave_profile("studio").unwrap();
        assert_eq!(std::fs::read_dir(dir.path().join("autosave")).unwrap().count(), 1);

        ws.save_profile("studio").unwrap();
        assert!(dir.path().join("profiles/studio.toml").exists());
        assert_eq!(std::fs::read_dir(dir.path().join("autosave")).unwrap().count(), 0);
    }

    #[test]
    fn unsaved_work_comes_back_after_the_app_closes() {
        let (dir, mut ws) = workspace();
        ws.profiles.get_mut("default").unwrap().name = "Changed".into();
        ws.autosave_profile("default").unwrap();
        ws.add_unsaved_profile(Profile { name: "Studio".into(), ..Profile::default() }).unwrap();
        std::fs::write(dir.path().join("autosave/broken_autosave.toml"), "rows = [not valid").unwrap();

        let mut again = Workspace::load(Paths::at(dir.path())).unwrap();
        assert_ne!(again.profiles["default"].name, "Changed", "only when asked for");
        let mut recovered = again.recover_autosaves();
        recovered.sort();
        assert_eq!(recovered, vec!["default".to_string(), "studio".to_string()]);
        assert_eq!(again.profiles["default"].name, "Changed");
        assert!(again.has_unsaved("studio"));
        assert!(again.errors.iter().any(|e| e.starts_with("broken_autosave.toml")), "{:?}", again.errors);
        assert!(dir.path().join("autosave/broken_autosave.toml").exists());
    }

    #[test]
    fn a_profile_file_with_a_mistake_is_never_saved_over() {
        let (dir, mut ws) = workspace();

        // Broken since the start: a new profile of the same name takes another file.
        let refused = dir.path().join("profiles/studio.toml");
        write_later(&refused, "version = 999\n[[pages]]\nname = \"x\"\n");
        ws.check_reload();
        let id = ws.add_profile(Profile::sample(false)).unwrap();
        assert_ne!(id, "studio");
        let mut studio = Profile::sample(false);
        studio.name = "Studio".into();
        assert_eq!(ws.add_profile(studio).unwrap(), "studio-2");
        assert!(std::fs::read_to_string(&refused).unwrap().contains("999"));

        // Broke after it was read: the last good copy doesn't replace it.
        let broken = dir.path().join("profiles/default.toml");
        write_later(&broken, "rows = [not valid");
        ws.check_reload();
        assert!(ws.save_profile("default").is_err());
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "rows = [not valid");
    }

    #[test]
    fn a_grants_file_with_a_mistake_is_reported_and_never_saved_over() {
        let (dir, mut ws) = workspace();
        let path = dir.path().join("grants.toml");
        write_later(&path, "[plugins.a\nenabled = true\n");
        ws.check_reload();
        assert!(ws.errors.iter().any(|e| e.starts_with("grants.toml")), "{:?}", ws.errors);
        assert!(crate::policy::Grants::allow_host(&ws.paths, "a", "b.example").is_err());
        assert!(std::fs::read_to_string(&path).unwrap().contains("[plugins.a\n"));

        write_later(&path, "");
        ws.check_reload();
        assert!(ws.errors.is_empty(), "{:?}", ws.errors);
    }

    #[test]
    fn newer_or_impossible_files_are_refused() {
        let (dir, mut ws) = workspace();
        write_later(&dir.path().join("profiles/a.toml"), "version = 999\n[[pages]]\nname = \"x\"\n");
        write_later(&dir.path().join("profiles/b.toml"), "rows = 0\n[[pages]]\nname = \"x\"\n");
        ws.check_reload();
        assert!(!ws.profiles.contains_key("a"));
        assert!(!ws.profiles.contains_key("b"));
        assert_eq!(ws.errors.len(), 2);
    }

    #[test]
    fn the_activity_record_stays_small_even_with_long_characters() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        for _ in 0..3000 {
            crate::paths::audit(&paths, "test", "日本語のログの行です。日本語のログの行です。日本語のログの行です。日本語のログの行です。");
        }
        assert!(std::fs::metadata(paths.audit()).unwrap().len() < 300 * 1024);
    }

    #[test]
    fn keys_saved_as_macros_keep_their_timing_and_approval() {
        let (dir, mut ws) = workspace();
        let steps = vec![
            Step { action: "a/b".into(), ..Step::default() },
            Step { action: "a/c".into(), ..Step::default() },
            Step { action: "a/d".into(), delay_ms: 90, ..Step::default() },
        ];
        let fingerprint = sha256_hex(format!("{}\0300", approval_hash(&steps)).as_bytes());
        let old_key = |col: u32, approved: &str| {
            let actions = ["a/b", "a/c"].map(|action| format!("[[pages.keys.steps]]\naction = \"{action}\"\n")).concat();
            format!(
                "[[pages.keys]]\nrow = 0\ncol = {col}\nis_macro = true\nmacro_delay_ms = 300\napproved = \"{approved}\"\n{actions}[[pages.keys.steps]]\naction = \"a/d\"\ndelay_ms = 90\n"
            )
        };
        let file = format!("[[pages]]\nname = \"Main\"\n{}{}", old_key(0, &fingerprint), old_key(1, "not-real"));
        write_later(&dir.path().join("profiles/old.toml"), &file);
        ws.check_reload();
        let keys = &ws.profiles["old"].pages[0].keys;
        let waits: Vec<u64> = keys[0].steps.iter().map(|step| step.delay_ms).collect();
        assert_eq!(waits, vec![0, 300, 90], "a step's own wait is kept");
        assert!(keys[0].is_approved(), "what was approved stays approved");
        assert!(!keys[1].is_approved(), "and nothing else becomes approved");

        ws.save_profile("old").unwrap();
        let saved = std::fs::read_to_string(dir.path().join("profiles/old.toml")).unwrap();
        assert!(!saved.contains("macro"), "{saved}");
    }

    #[test]
    fn profile_names_become_safe_file_names() {
        assert_eq!(profile_id("Studio Mix #2"), "studio-mix-2");
        assert_eq!(profile_id("../../etc/passwd"), "etc-passwd");
        assert_eq!(profile_id("!!!"), "profile");
    }

    fn setting_names(value: &serde_json::Value, names: &mut Vec<String>) {
        for (name, value) in value.as_object().into_iter().flatten() {
            names.push(name.clone());
            setting_names(value, names);
        }
    }

    #[test]
    fn the_config_guide_shows_the_real_defaults() {
        let guide = include_str!("../../../docs/CONFIG.md");
        let block = guide.split_once("```toml\n").unwrap().1.split_once("\n```").unwrap().0;
        let mut shown: Settings = toml::from_str(block).unwrap();
        assert_eq!(shown.presets.len(), 1, "the example preset is read too");
        shown.presets.clear();
        assert_eq!(serde_json::to_value(shown).unwrap(), serde_json::to_value(Settings::default()).unwrap());
    }

    #[test]
    fn every_setting_is_in_the_config_guide() {
        let guide = include_str!("../../../docs/CONFIG.md");
        let mut names = Vec::new();
        setting_names(&serde_json::to_value(Settings::default()).unwrap(), &mut names);
        // Every field written out, so a new one won't compile until it's been thought about here.
        let key = Key {
            row: 0,
            col: 0,
            title: String::new(),
            title_position: String::new(),
            icon: String::new(),
            background: String::new(),
            color: String::new(),
            icon_color: String::new(),
            text_color: String::new(),
            toggle: false,
            alt_title: String::new(),
            alt_color: String::new(),
            steps: Vec::new(),
            approved: String::new(),
            folder: None,
            old_macro_delay_ms: 0,
        };
        setting_names(&serde_json::to_value(key).unwrap(), &mut names);
        assert!(names.len() > 25, "{names:?}");
        for name in names {
            assert!(guide.contains(&format!("{name} =")) || guide.contains(&format!("`{name}`")) || guide.contains(&format!("[{name}]")), "{name} isn't in CONFIG.md");
        }
    }

    #[test]
    fn a_save_keeps_a_change_another_program_made_to_the_same_file() {
        let (dir, mut ws) = workspace();
        let path = dir.path().join("profiles/default.toml");
        // Another program (the MCP server) writes the file while we are editing our copy.
        let mut theirs = ws.profiles["default"].clone();
        theirs.pages[0].keys.push(Key {
            row: 2,
            col: 3,
            title: "Theirs".into(),
            ..Key::default()
        });
        std::fs::write(&path, super::to_toml(&theirs).unwrap()).unwrap();
        // We change a key of our own.
        ws.profiles.get_mut("default").unwrap().pages[0].keys[0].title = "Mine".into();
        ws.save_profile("default").unwrap();
        let saved: Profile = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let titles: Vec<&str> = saved.pages[0].keys.iter().map(|k| k.title.as_str()).collect();
        assert!(titles.contains(&"Mine"), "our change is kept: {titles:?}");
        assert!(titles.contains(&"Theirs"), "and so is theirs: {titles:?}");
    }

    #[test]
    fn deleted_files_disappear_and_active_moves() {
        let (_dir, mut ws) = workspace();
        let id = ws.add_profile(Profile { name: "Two".into(), ..Profile::default() }).unwrap();
        ws.settings.active_profile = id.clone();
        ws.delete_profile(&id).unwrap();
        assert_eq!(ws.settings.active_profile, "default");
        assert!(ws.delete_profile("default").is_err(), "keeps the last one");
    }
}
