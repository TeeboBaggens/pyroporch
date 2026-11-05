//! Exporting and importing config as one file, and storing key pictures.
//!
//! A file from somewhere else is treated as untrusted. Importing it can't approve a key, allow a
//! site or bring in a credential. An exported file holds no secrets, only references to them, so
//! it's safe to share: whoever imports it enters their own.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::config::{Key, Preset, Profile, UiSettings, Workspace};
use crate::engine::{Block, Engine};
use crate::paths::{Paths, write_private};
use crate::security::{find_refs, plaintext_findings, sha256_hex};

pub const FILE_EXTENSION: &str = "pyroporch";
const FORMAT: &str = "pyroporch-config";
const VERSION: u32 = 1;
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bundle {
    pub format: String,
    pub version: u32,
    pub ui: Option<UiSettings>,
    pub presets: Vec<Preset>,
    pub profiles: BTreeMap<String, Profile>,
    /// Key pictures, by file name.
    pub images: BTreeMap<String, String>,
}

/// What importing did, in terms the person can act on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub profiles: Vec<String>,
    pub presets: usize,
    pub images: usize,
    /// Keys left out because they held a credential in plain text.
    pub removed_keys: usize,
    /// Keys that act on the computer, waiting for approval.
    pub need_approval: usize,
    /// Vault secrets the imported keys refer to that aren't stored here yet.
    pub missing_secrets: Vec<String>,
    /// Sites the imported keys call that aren't allowed here yet.
    pub sites: Vec<String>,
    /// The plugin each of those sites is called by, so "allowed" is judged for that plugin.
    pub called: Vec<(String, String)>,
}

/// Saves a PNG in the images folder, named after its contents, and returns that name.
pub fn store_image(paths: &Paths, png: &[u8]) -> Result<String, String> {
    if !png.starts_with(PNG_MAGIC) {
        return Err("that isn't a PNG file".into());
    }
    if png.len() > MAX_IMAGE_BYTES {
        return Err("that picture is too big".into());
    }
    let name = format!("{}.png", &sha256_hex(png)[..32]);
    let path = paths.image(&name).ok_or("bad image name")?;
    if !path.exists() {
        write_private(&path, png).map_err(|e| e.to_string())?;
    }
    Ok(name)
}

/// Makes the export file: one profile, or everything when `only` is `None`.
pub fn export(ws: &Workspace, engine: &Engine, only: Option<&str>) -> Result<String, String> {
    let mut bundle = Bundle {
        format: FORMAT.into(),
        version: VERSION,
        ..Bundle::default()
    };
    match only {
        Some(id) => {
            let profile = ws.profiles.get(id).ok_or("no such profile")?;
            bundle.profiles.insert(id.to_string(), profile.clone());
        }
        None => {
            bundle.profiles = ws.profiles.clone();
            bundle.presets = ws.settings.presets.iter().filter(|preset| !preset_holds_credential(engine, preset)).cloned().collect();
            bundle.ui = Some(ws.settings.ui.clone());
        }
    }
    let mut names = BTreeSet::new();
    for profile in bundle.profiles.values_mut() {
        for page in &mut profile.pages {
            page.for_each_key_mut(&mut |key| {
                // Approval is the person's decision on this computer, so it doesn't travel.
                key.approved.clear();
                if !key.background.is_empty() {
                    names.insert(key.background.clone());
                }
            });
        }
    }
    // An exported file holds no secrets, so a key that has one in plain text is refused
    // rather than written out. That is the same rule the import follows.
    for profile in bundle.profiles.values() {
        let mut offending = Vec::new();
        for page in &profile.pages {
            page.for_each_key(&mut |key| {
                if holds_credential(engine, key) {
                    offending.push(if key.title.is_empty() { format!("row {}, col {}", key.row, key.col) } else { key.title.clone() });
                }
            });
        }
        if !offending.is_empty() {
            return Err(format!(
                "these keys hold a password in plain text, so they can't be exported: {}",
                offending.join(", ")
            ));
        }
    }
    for name in names {
        if let Some(bytes) = ws.paths.image(&name).filter(|path| path.is_file()).and_then(|path| std::fs::read(path).ok()) {
            bundle.images.insert(name, B64.encode(bytes));
        }
    }
    serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())
}

/// True when a key's settings hold a credential in plain text.
fn holds_credential(engine: &Engine, key: &Key) -> bool {
    key.steps.iter().any(|step| {
        // By the action's own form if it's installed, by general rules if it isn't.
        engine.check_step(step).iter().any(|block| matches!(block, Block::PlainCredential(_)))
            || !plaintext_findings(None, &step.settings).is_empty()
    })
}

/// The same for a preset, which goes through the same checks as a key's step.
fn preset_holds_credential(engine: &Engine, preset: &crate::config::Preset) -> bool {
    let step = crate::config::Step {
        action: preset.action.clone(),
        settings: preset.settings.clone(),
        delay_ms: 0,
    };
    engine.check_step(&step).iter().any(|block| matches!(block, Block::PlainCredential(_)))
        || !plaintext_findings(None, &step.settings).is_empty()
}

/// Reads an export file and checks it's one, without changing anything.
pub fn read(text: &str) -> Result<Bundle, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err("that file is too big".into());
    }
    let mut bundle: Bundle =
        serde_json::from_str(text).map_err(|_| "that isn't a Pyro Porch export file".to_string())?;
    if bundle.format != FORMAT {
        return Err("that isn't a Pyro Porch export file".into());
    }
    if bundle.version > VERSION {
        return Err("that file was made by a newer version of Pyro Porch".into());
    }
    for profile in bundle.profiles.values_mut() {
        profile.validate().map_err(|e| format!("{}: {e}", profile.name))?;
        profile.upgrade_old_fields();
    }
    Ok(bundle)
}

/// Removes keys holding a plain credential, and anything else that shouldn't arrive with the file.
fn clean_keys(keys: &mut Vec<Key>, engine: &Engine, images: &BTreeMap<String, String>, report: &mut ImportReport) {
    let before = keys.len();
    keys.retain(|key| !holds_credential(engine, key));
    report.removed_keys += before - keys.len();
    for key in keys.iter_mut() {
        key.approved.clear();
        if engine.check_key(key).contains(&Block::NeedsApproval) {
            report.need_approval += 1;
        }
        key.background = images.get(&key.background).cloned().unwrap_or_default();
        for (plugin, site) in engine.sites_called(key) {
            if !report.sites.contains(&site) {
                report.sites.push(site.clone());
            }
            report.called.push((plugin, site));
        }
        for step in &key.steps {
            for value in step.settings.values() {
                let mut names = Vec::new();
                collect_secret_names(value, &mut names);
                for name in names {
                    if !report.missing_secrets.contains(&name) {
                        report.missing_secrets.push(name);
                    }
                }
            }
        }
        if let Some(folder) = &mut key.folder {
            clean_keys(&mut folder.keys, engine, images, report);
        }
    }
}

fn collect_secret_names(value: &serde_json::Value, names: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => names.extend(find_refs(text).into_iter().map(|r| r.name)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_secret_names(v, names)),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_secret_names(v, names)),
        _ => {}
    }
}

/// Adds what's in the file. Existing profiles are never overwritten: an import with the same name
/// arrives alongside as a new one.
pub fn apply(ws: &mut Workspace, engine: &Engine, mut bundle: Bundle, use_appearance: bool) -> Result<ImportReport, String> {
    let mut report = ImportReport::default();

    // Picture names are recomputed from the contents, so the file can't choose where they land.
    let mut stored = BTreeMap::new();
    for (name, encoded) in &bundle.images {
        let Ok(bytes) = B64.decode(encoded) else {
            continue;
        };
        if let Ok(new_name) = store_image(&ws.paths, &bytes) {
            stored.insert(name.clone(), new_name);
            report.images += 1;
        }
    }

    for (_, mut profile) in std::mem::take(&mut bundle.profiles) {
        for page in &mut profile.pages {
            clean_keys(&mut page.keys, engine, &stored, &mut report);
        }
        report.profiles.push(ws.add_profile(profile)?);
    }

    for preset in std::mem::take(&mut bundle.presets) {
        if preset_holds_credential(engine, &preset) {
            report.removed_keys += 1;
            continue;
        }
        // Like a profile, one with the same name arrives alongside it, unless it's the same one.
        let same = |existing: &crate::config::Preset| existing.name == preset.name && existing.action == preset.action && existing.settings == preset.settings;
        if ws.settings.presets.iter().any(same) {
            continue;
        }
        let mut preset = preset;
        let base = preset.name.clone();
        let mut n = 2;
        while ws.settings.presets.iter().any(|existing| existing.name == preset.name) {
            preset.name = format!("{base} ({n})");
            n += 1;
        }
        ws.settings.presets.push(preset);
        report.presets += 1;
    }
    if use_appearance && let Some(ui) = bundle.ui {
        ws.settings.ui = ui;
    }
    ws.save_settings()?;

    let vault = engine.vault.lock().map_err(|_| "the vault is unavailable")?;
    report.missing_secrets.retain(|name| !vault.contains(name));
    report.missing_secrets.sort();
    // A site is only allowed if the person allowed it for the plugin that calls it.
    let grants = crate::policy::Grants::load(&ws.paths);
    let mut sites: Vec<String> = Vec::new();
    for (plugin, site) in &report.called {
        if !grants.allows_host(plugin, site) && !sites.contains(site) {
            sites.push(site.clone());
        }
    }
    report.sites = sites;
    report.sites.sort();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Page, Step};
    use crate::vault::{KdfParams, Vault};
    use pyroporch_sdk::{ActionSpec, Field, FieldKind, HostHandle, Outcome, Plugin, PluginInfo, Risk, RunRequest};
    use serde_json::json;
    use std::sync::Arc;

    struct Fake;
    impl Plugin for Fake {
        fn info(&self) -> PluginInfo {
            let action = |id: &str, risk, fields| ActionSpec {
                id: id.into(),
                name: id.into(),
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
                    action("risky", Risk::High, vec![]),
                ],
            }
        }
        fn run(&self, _: &RunRequest, _: &HostHandle) -> Result<Outcome, String> {
            Ok(Outcome::default())
        }
    }

    fn setup() -> (tempfile::TempDir, Workspace, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        let ws = Workspace::load(paths.clone()).unwrap();
        let vault = Vault::with_kdf(paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
        let engine = Engine::new(paths, vault, vec![Arc::new(Fake)]);
        (dir, ws, engine)
    }

    fn step(action: &str, settings: serde_json::Value) -> Step {
        Step {
            action: action.into(),
            settings: settings.as_object().unwrap().clone(),
            delay_ms: 0,
        }
    }

    fn bundle_of(profile: Profile) -> Bundle {
        Bundle {
            format: FORMAT.into(),
            version: VERSION,
            profiles: BTreeMap::from([("x".to_string(), profile)]),
            ..Bundle::default()
        }
    }

    fn one_page(name: &str, keys: Vec<Key>) -> Profile {
        Profile {
            name: name.into(),
            pages: vec![Page { name: "Main".into(), keys }],
            ..Profile::default()
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really a picture, but it starts like one";

    fn profile_with_everything(image: &str) -> Profile {
        let inner = Key { row: 0, col: 1, title: "Inner".into(), steps: vec![step("test.fake/risky", json!({}))], ..Key::default() };
        let mut outer = vec![
            Key {
                row: 0,
                col: 0,
                title: "Call".into(),
                background: image.into(),
                steps: vec![step("test.fake/call", json!({"url": "https://api.example.com/x", "secret": "{{secret:svc_key}}"}))],
                ..Key::default()
            },
            Key { row: 0, col: 1, title: "Folder".into(), folder: Some(Box::new(Page { name: "F".into(), keys: vec![inner] })), ..Key::default() },
        ];
        outer[1].approve();
        one_page("Shared", outer)
    }

    #[test]
    fn a_round_trip_brings_keys_folders_and_pictures_across() {
        let (_a, mut source, source_engine) = setup();
        let image = store_image(&source.paths, PNG).unwrap();
        let id = source.add_profile(profile_with_everything(&image)).unwrap();
        let text = export(&source, &source_engine, Some(&id)).unwrap();

        let (_b, mut target, engine) = setup();
        let report = apply(&mut target, &engine, read(&text).unwrap(), false).unwrap();

        let imported = &target.profiles[&report.profiles[0]];
        assert_eq!(imported.name, "Shared");
        let keys = &imported.pages[0].keys;
        assert_eq!(keys[1].folder.as_ref().unwrap().keys[0].title, "Inner");
        assert_eq!(report.images, 1);
        assert!(target.paths.image(&keys[0].background).unwrap().exists());
        assert_eq!(report.missing_secrets, vec!["svc_key"]);
        assert_eq!(report.sites, vec!["api.example.com"]);
        assert_eq!(report.need_approval, 1, "the risky key inside the folder");
    }

    #[test]
    fn an_export_holds_no_secret_or_approval() {
        let (_a, mut ws, engine) = setup();
        engine.vault.lock().unwrap().create("correct horse battery").unwrap();
        engine.vault.lock().unwrap().set("svc_key", "VALUE-MUST-NOT-TRAVEL", &["api.example.com".into()]).unwrap();
        let mut profile = profile_with_everything("");
        profile.pages[0].keys[0].approve();
        let id = ws.add_profile(profile).unwrap();
        let text = export(&ws, &engine, Some(&id)).unwrap();
        assert!(!text.contains("VALUE-MUST-NOT-TRAVEL"));
        assert!(text.contains("{{secret:svc_key}}"));
        let bundle = read(&text).unwrap();
        bundle.profiles.values().next().unwrap().pages[0].for_each_key(&mut |key| assert!(key.approved.is_empty()));
    }

    #[test]
    fn an_import_never_replaces_a_preset_of_the_same_name() {
        let (_dir, mut ws, engine) = setup();
        let preset = |url: &str| {
            let mut settings = serde_json::Map::new();
            settings.insert("url".into(), json!(url));
            crate::config::Preset { name: "Mine".into(), action: "test.fake/call".into(), settings, ..Default::default() }
        };
        ws.settings.presets.push(preset("https://a.example/"));
        let bundle = Bundle { format: FORMAT.into(), version: VERSION, presets: vec![preset("https://b.example/"), preset("https://a.example/")], ..Bundle::default() };
        apply(&mut ws, &engine, bundle, false).unwrap();
        let names: Vec<&str> = ws.settings.presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Mine", "Mine (2)"]);
        assert_eq!(ws.settings.presets[0].settings["url"], "https://a.example/");
    }

    #[test]
    fn a_preset_with_a_plain_credential_stays_out_of_the_export() {
        let (_dir, mut ws, engine) = setup();
        let mut settings = serde_json::Map::new();
        settings.insert("password".into(), json!("hunter2"));
        ws.settings.presets.push(crate::config::Preset {
            name: "Leaky".into(),
            action: "test.fake/call".into(),
            settings,
            ..Default::default()
        });
        let text = export(&ws, &engine, None).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("Leaky"), "{text}");
    }

    #[test]
    fn a_forged_approval_in_a_file_does_not_count() {
        let (_a, mut ws, engine) = setup();
        let steps = vec![step("test.fake/risky", json!({}))];
        let mut forged = Key { row: 0, col: 0, title: "Sneaky".into(), steps, ..Key::default() };
        forged.approve();
        let text = serde_json::to_string(&bundle_of(one_page("Forged", vec![forged]))).unwrap();
        let report = apply(&mut ws, &engine, read(&text).unwrap(), false).unwrap();
        let key = &ws.profiles[&report.profiles[0]].pages[0].keys[0];
        assert!(!key.is_approved());
        assert_eq!(engine.check_key(key), vec![Block::NeedsApproval]);
    }

    #[test]
    fn plain_credentials_in_a_file_never_land_on_disk() {
        let (dir, mut ws, engine) = setup();
        let leaky = Key {
            row: 0,
            col: 0,
            title: "Leaky".into(),
            steps: vec![step("test.fake/call", json!({"secret": "hunter2-in-a-file"}))],
            ..Key::default()
        };
        let unknown_action = Key {
            row: 1,
            col: 0,
            title: "Unknown".into(),
            steps: vec![step("not.installed/x", json!({"password": "hunter3-in-a-file"}))],
            ..Key::default()
        };
        let clean = Key { row: 0, col: 1, title: "Clean".into(), steps: vec![step("test.fake/call", json!({"url": "https://a.example/"}))], ..Key::default() };
        let text = serde_json::to_string(&bundle_of(one_page("Leaky", vec![leaky, unknown_action, clean]))).unwrap();
        let report = apply(&mut ws, &engine, read(&text).unwrap(), false).unwrap();
        assert_eq!(report.removed_keys, 2);
        let keys = &ws.profiles[&report.profiles[0]].pages[0].keys;
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].title, "Clean");
        let saved = std::fs::read_to_string(dir.path().join(format!("profiles/{}.toml", report.profiles[0]))).unwrap();
        assert!(!saved.contains("hunter"));
    }

    #[test]
    fn a_file_cannot_choose_where_its_pictures_land() {
        let (dir, mut ws, engine) = setup();
        let key = Key { row: 0, col: 0, background: "../../evil.png".into(), ..Key::default() };
        let mut bundle = bundle_of(one_page("P", vec![key]));
        bundle.images = BTreeMap::from([
            ("../../evil.png".to_string(), B64.encode(PNG)),
            ("script.png".to_string(), B64.encode(b"#!/bin/sh not a png")),
        ]);
        let report = apply(&mut ws, &engine, read(&serde_json::to_string(&bundle).unwrap()).unwrap(), false).unwrap();
        assert_eq!(report.images, 1, "only the real PNG");
        assert!(!dir.path().join("../evil.png").exists());
        let name = &ws.profiles[&report.profiles[0]].pages[0].keys[0].background;
        assert!(!name.contains(".."), "{name}");
        assert!(ws.paths.image(name).unwrap().starts_with(ws.paths.images_dir()));
    }

    #[test]
    fn an_import_never_overwrites_and_appearance_is_optional() {
        let (_a, mut ws, engine) = setup();
        let mut source_ui = ws.settings.ui.clone();
        source_ui.theme = "light".into();
        let bundle = Bundle {
            format: FORMAT.into(),
            version: VERSION,
            ui: Some(source_ui),
            profiles: BTreeMap::from([("default".to_string(), Profile { name: "Default".into(), ..Profile::default() })]),
            ..Bundle::default()
        };
        let text = serde_json::to_string(&bundle).unwrap();
        let before = ws.profiles["default"].clone();
        let report = apply(&mut ws, &engine, read(&text).unwrap(), false).unwrap();
        assert_ne!(report.profiles[0], "default");
        assert_eq!(ws.profiles["default"], before);
        assert_eq!(ws.settings.ui.theme, "dark");
        apply(&mut ws, &engine, read(&text).unwrap(), true).unwrap();
        assert_eq!(ws.settings.ui.theme, "light");
    }

    #[test]
    fn other_files_are_refused() {
        assert!(read("not json").is_err());
        assert!(read("{\"format\": \"something-else\"}").is_err());
        assert!(read(&format!("{{\"format\": \"{FORMAT}\", \"version\": 999}}")).is_err());
    }
}
