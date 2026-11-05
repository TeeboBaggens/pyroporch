//! The sample plugins: API Calls and Screen Saver. The app carries them, puts them in the plugins
//! folder the first time, and from there on they're plugins like any other. They run in the
//! sandbox, people can read how they're set up, and removing the folder removes them for good,
//! unless the person asks for them back.

use crate::config::Workspace;
use crate::engine::Manifest;
use crate::paths::{Paths, write_private};
use crate::policy::Grants;
use crate::security::sha256_hex;

struct Sample {
    folder: &'static str,
    id: &'static str,
    manifest: &'static str,
    wasm: &'static [u8],
    source: &'static str,
}

const SAMPLES: [Sample; 2] = [
    Sample {
        folder: "api-call",
        id: "pyroporch.api-call",
        manifest: include_str!("../../../plugins/api-call/plugin.toml"),
        wasm: include_bytes!("../../../plugins/api-call/plugin.wasm"),
        source: include_str!("../../../plugins/_examples/api-call/src/lib.rs"),
    },
    Sample {
        folder: "screensaver",
        id: "pyroporch.screensaver",
        manifest: include_str!("../../../plugins/screensaver/plugin.toml"),
        wasm: include_bytes!("../../../plugins/screensaver/plugin.wasm"),
        source: include_str!("../../../plugins/_examples/screensaver/src/lib.rs"),
    },
];

const README: &str = "This is a sample plugin that came with Pyro Porch.

  plugin.toml      what the plugin is, and what it asks to be allowed to do
  plugin.wasm      the built plugin, which runs in a sandbox
  source/lib.rs    the code it was built from, to read and learn from

Don't want it? Remove it from the app's Plugins screen, or delete this folder. Changing the plugin
file means approving the plugin again. To write your own, see README.md, one folder up.
";

/// The guide to writing a plugin, kept up to date in the plugins folder, where people look for it.
const GUIDE: &str = include_str!("../../../plugins/README.md");
pub const GUIDE_FILE: &str = "README.md";
/// What the guide was called before, which this replaces.
const OLD_GUIDE_FILE: &str = "PLUGIN-GUIDE.md";

/// A sample the app used to put in the folder that's now part of the app itself: its folder name and id.
const RETIRED: (&str, &str) = ("desktop", "pyroporch.desktop");

/// Takes a retired sample back out, if it's still as the app left it. Anything else there is someone's own.
fn clear_retired(ws: &mut Workspace) -> Result<bool, String> {
    let (folder, id) = RETIRED;
    let Some(recorded) = ws.settings.sample_hashes.remove(folder) else {
        return Ok(false);
    };
    let path = ws.paths.plugins_dir().join(folder);
    let on_disk = hash_of_plain_file(&path.join("plugin.wasm"));
    if on_disk.as_ref() == Some(&recorded) {
        std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    }
    let mut grants = Grants::try_load(&ws.paths)?;
    if grants.plugins.remove(id).is_some() {
        grants.save(&ws.paths)?;
    }
    Ok(true)
}

pub fn is_sample(id: &str) -> bool {
    SAMPLES.iter().any(|sample| sample.id == id)
}

fn write_files(paths: &Paths, sample: &Sample) -> Result<(), String> {
    let folder = paths.plugins_dir().join(sample.folder);
    let fail = |e: std::io::Error| e.to_string();
    write_private(&folder.join("plugin.toml"), sample.manifest.as_bytes()).map_err(fail)?;
    write_private(&folder.join("plugin.wasm"), sample.wasm).map_err(fail)?;
    write_private(&folder.join("source/lib.rs"), sample.source.as_bytes()).map_err(fail)?;
    write_private(&folder.join("README.txt"), README.as_bytes()).map_err(fail)
}

/// The app vouches for what it carries, so a sample starts out approved. Any sites the person has
/// allowed already stay allowed.
fn approve(paths: &Paths, sample: &Sample, keep_off: bool) -> Result<(), String> {
    let manifest: Manifest = toml::from_str(sample.manifest).map_err(|e| e.message().to_string())?;
    let mut grants = Grants::try_load(paths)?;
    let grant = grants.plugins.entry(sample.id.to_string()).or_default();
    grant.enabled = !keep_off || grant.enabled;
    grant.sha256 = sha256_hex(sample.wasm);
    grant.secrets = manifest.permissions.secrets;
    grant.deck = manifest.permissions.deck;
    for host in manifest.permissions.http_hosts {
        if !grant.http_hosts.contains(&host) {
            grant.http_hosts.push(host);
        }
    }
    grants.save(paths)
}

/// `keep_off` is for an update: one the person turned off stays off.
fn install(ws: &mut Workspace, sample: &Sample, keep_off: bool) -> Result<(), String> {
    write_files(&ws.paths, sample)?;
    approve(&ws.paths, sample, keep_off)?;
    ws.settings.sample_hashes.insert(sample.folder.to_string(), sha256_hex(sample.wasm));
    Ok(())
}

/// Reading a pipe or a device could wait forever, so only a plain file is looked at.
fn hash_of_plain_file(path: &std::path::Path) -> Option<String> {
    path.is_file().then(|| std::fs::read(path).ok()).flatten().map(|bytes| sha256_hex(&bytes))
}

/// Whether the two files that record the samples can be read, and so safely saved.
fn can_record_changes(ws: &Workspace) -> Result<(), String> {
    if ws.settings_file_broken() {
        return Err("config.toml has a mistake that needs fixing first".into());
    }
    Grants::try_load(&ws.paths).map(|_| ())
}

/// Run at each start. Adds samples that have never been installed, and updates ones the person
/// hasn't changed. A sample that was removed stays removed.
pub fn sync(ws: &mut Workspace) -> Result<(), String> {
    let guide = ws.paths.plugins_dir().join(GUIDE_FILE);
    if std::fs::read_to_string(&guide).ok().as_deref() != Some(GUIDE) {
        write_private(&guide, GUIDE.as_bytes()).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(ws.paths.plugins_dir().join(OLD_GUIDE_FILE));
    // Without these files there's no telling what the person chose, so nothing is decided for
    // them. The app still starts and shows what's wrong, and the next start catches up.
    if can_record_changes(ws).is_err() {
        return Ok(());
    }
    let mut changed = clear_retired(ws)?;
    for sample in &SAMPLES {
        let carried = sha256_hex(sample.wasm);
        let folder = ws.paths.plugins_dir().join(sample.folder);
        // Its note points at the guide, so it moves with it. A plugin's approval doesn't cover this.
        let note = folder.join("README.txt");
        if note.is_file() && std::fs::read_to_string(&note).ok().as_deref() != Some(README) {
            write_private(&note, README.as_bytes()).map_err(|e| e.to_string())?;
        }
        let on_disk = hash_of_plain_file(&folder.join("plugin.wasm"));
        let recorded = ws.settings.sample_hashes.get(sample.folder);
        let updating = recorded.is_some();
        let due = match recorded {
            // A folder of this name that the app didn't put there is someone's own.
            None => !folder.exists(),
            // Updated only if it's still as the app left it. Removed stays removed.
            Some(recorded) => on_disk.as_ref() == Some(recorded) && *recorded != carried,
        };
        if due {
            install(ws, sample, updating)?;
            changed = true;
        }
    }
    if changed {
        ws.save_settings()?;
    }
    Ok(())
}

/// The samples that aren't in the plugins folder right now.
pub fn missing(paths: &Paths) -> Vec<&'static str> {
    SAMPLES
        .iter()
        .filter(|sample| !paths.plugins_dir().join(sample.folder).exists())
        .map(|sample| sample.folder)
        .collect()
}

/// Puts back any samples that were removed.
pub fn restore(ws: &mut Workspace) -> Result<(), String> {
    can_record_changes(ws)?;
    for sample in &SAMPLES {
        if !ws.paths.plugins_dir().join(sample.folder).exists() {
            install(ws, sample, false)?;
        }
    }
    ws.save_settings()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Key, Step};
    use crate::engine::Engine;
    use crate::vault::{KdfParams, Vault};
    use pyroporch_sdk::{PaintRequest, Trigger};

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::load(Paths::at(dir.path())).unwrap();
        (dir, ws)
    }

    fn engine_for(ws: &Workspace) -> Engine {
        let vault = Vault::with_kdf(ws.paths.vault(), KdfParams { memory_kib: 1024, passes: 1, lanes: 1 }).unwrap();
        Engine::new(ws.paths.clone(), vault, vec![])
    }

    #[test]
    fn the_first_start_puts_both_samples_in_the_folder_ready_to_use() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        for folder in ["api-call", "screensaver"] {
            for file in ["plugin.toml", "plugin.wasm", "source/lib.rs", "README.txt"] {
                assert!(dir.path().join("plugins").join(folder).join(file).exists(), "{folder}/{file}");
            }
        }
        let engine = engine_for(&ws);
        let statuses = engine.plugin_statuses();
        for id in ["pyroporch.api-call", "pyroporch.screensaver"] {
            let status = statuses.iter().find(|s| s.id == id).unwrap();
            assert!(status.enabled, "{id}: {}", status.note);
            assert!(status.sample && !status.builtin, "{id}");
        }
        assert!(engine.spec("pyroporch.api-call/request").is_some());
        assert!(engine.spec("pyroporch.screensaver/toggle").is_some());
    }

    #[test]
    fn the_guide_is_in_the_plugins_folder_and_kept_up_to_date_without_looking_like_a_plugin() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let guide = dir.path().join("plugins").join(GUIDE_FILE);
        assert_eq!(std::fs::read_to_string(&guide).unwrap(), GUIDE);
        std::fs::write(&guide, "old").unwrap();
        sync(&mut ws).unwrap();
        assert_eq!(std::fs::read_to_string(&guide).unwrap(), GUIDE);
        let engine = engine_for(&ws);
        assert!(engine.plugin_statuses().iter().all(|s| s.folder != GUIDE_FILE));
    }

    #[test]
    fn the_desktop_sample_is_taken_back_out_now_its_part_of_the_app() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let folder = dir.path().join("plugins/desktop");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("plugin.wasm"), b"as the app left it").unwrap();
        ws.settings.sample_hashes.insert("desktop".into(), sha256_hex(b"as the app left it"));
        Grants::allow_host(&ws.paths, "pyroporch.desktop", "a.example").unwrap();

        sync(&mut ws).unwrap();
        assert!(!folder.exists());
        assert!(!ws.settings.sample_hashes.contains_key("desktop"));
        assert!(!Grants::load(&ws.paths).plugins.contains_key("pyroporch.desktop"));

        // A folder of that name the app didn't put there is left alone, even if it was changed since.
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("plugin.wasm"), b"someone's own").unwrap();
        sync(&mut ws).unwrap();
        assert!(folder.exists());
        ws.settings.sample_hashes.insert("desktop".into(), sha256_hex(b"as the app left it"));
        sync(&mut ws).unwrap();
        assert!(folder.exists(), "it isn't as the app left it");
    }

    #[test]
    fn the_guide_under_its_old_name_makes_way_for_the_readme() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let plugins = dir.path().join("plugins");
        std::fs::write(plugins.join(OLD_GUIDE_FILE), "old guide").unwrap();
        std::fs::write(plugins.join("api-call/README.txt"), "see PLUGIN-GUIDE.md").unwrap();
        sync(&mut ws).unwrap();
        assert!(!plugins.join(OLD_GUIDE_FILE).exists());
        assert_eq!(std::fs::read_to_string(plugins.join("api-call/README.txt")).unwrap(), README);
        assert!(engine_for(&ws).plugin_statuses().iter().any(|s| s.id == "pyroporch.api-call" && s.enabled));
    }

    #[test]
    fn an_update_doesnt_turn_a_sample_back_on() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        engine_for(&ws).disable_plugin("pyroporch.api-call").unwrap();

        // An older copy, as the app left it, so the next start updates it.
        let file = dir.path().join("plugins/api-call/plugin.wasm");
        std::fs::write(&file, b"older").unwrap();
        ws.settings.sample_hashes.insert("api-call".into(), sha256_hex(b"older"));
        ws.save_settings().unwrap();
        sync(&mut ws).unwrap();
        assert_ne!(std::fs::read(&file).unwrap(), b"older", "it was updated");
        assert!(!Grants::load(&ws.paths).plugins["pyroporch.api-call"].enabled);
    }

    #[test]
    fn a_config_file_that_cant_be_read_doesnt_bring_samples_back_or_get_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let text = "[security]\nauto_lock_minutes = \"ten\"\n";
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        let mut ws = Workspace::load(Paths::at(dir.path())).unwrap();
        sync(&mut ws).unwrap();
        assert!(!dir.path().join("plugins/api-call").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap(), text);
        assert!(restore(&mut ws).is_err());
    }

    #[test]
    fn a_removed_sample_stays_gone_until_asked_for_and_keeps_the_sites_you_allowed() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        Grants::allow_host(&ws.paths, "pyroporch.api-call", "api.example.com").unwrap();
        let engine = engine_for(&ws);
        engine.remove_plugin("pyroporch.api-call", "api-call").unwrap();
        assert!(!dir.path().join("plugins/api-call").exists());
        assert!(engine.spec("pyroporch.api-call/request").is_none(), "gone straight away");
        assert_eq!(missing(&ws.paths), vec!["api-call"]);

        // A new start doesn't bring it back.
        let mut again = Workspace::load(Paths::at(dir.path())).unwrap();
        sync(&mut again).unwrap();
        assert!(!dir.path().join("plugins/api-call").exists());

        restore(&mut again).unwrap();
        let engine = engine_for(&again);
        assert!(engine.spec("pyroporch.api-call/request").is_some());
        assert!(missing(&again.paths).is_empty());
        assert!(Grants::load(&again.paths).plugins["pyroporch.api-call"].http_hosts.contains(&"api.example.com".to_string()));
    }

    #[test]
    fn an_edited_sample_needs_approving_and_is_not_overwritten() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let file = dir.path().join("plugins/screensaver/plugin.wasm");
        let mut bytes = std::fs::read(&file).unwrap();
        bytes.extend_from_slice(b"\0\x04mine");
        std::fs::write(&file, &bytes).unwrap();

        let mut again = Workspace::load(Paths::at(dir.path())).unwrap();
        sync(&mut again).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "left as the person made it");
        let engine = engine_for(&again);
        assert!(engine.spec("pyroporch.screensaver/toggle").is_none());
        let status = engine.plugin_statuses().into_iter().find(|s| s.id == "pyroporch.screensaver").unwrap();
        assert!(status.note.contains("changed"), "{}", status.note);
    }

    #[test]
    fn an_old_untouched_sample_is_updated_by_a_new_version() {
        let (dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        // As if an older version of the app had installed an older copy.
        let file = dir.path().join("plugins/api-call/plugin.wasm");
        std::fs::write(&file, b"older").unwrap();
        ws.settings.sample_hashes.insert("api-call".into(), sha256_hex(b"older"));
        ws.save_settings().unwrap();

        let mut again = Workspace::load(Paths::at(dir.path())).unwrap();
        sync(&mut again).unwrap();
        assert_ne!(std::fs::read(&file).unwrap(), b"older");
        assert!(engine_for(&again).spec("pyroporch.api-call/request").is_some());
    }

    #[test]
    fn someone_elses_folder_with_the_same_name_is_left_alone() {
        let (dir, mut ws) = workspace();
        let theirs = dir.path().join("plugins/screensaver");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("notes.txt"), "mine").unwrap();
        sync(&mut ws).unwrap();
        assert!(!theirs.join("plugin.toml").exists());
        assert!(dir.path().join("plugins/api-call/plugin.toml").exists());
    }

    #[test]
    fn the_api_sample_can_use_any_secret_and_still_cannot_send_one_elsewhere() {
        let (_dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let engine = engine_for(&ws);
        Grants::allow_host(&ws.paths, "pyroporch.api-call", "api.other.example").unwrap();
        engine.vault.lock().unwrap().create("correct horse battery").unwrap();
        engine.vault.lock().unwrap().set("token", "value", &["api.example.com".into()]).unwrap();
        let key = Key {
            steps: vec![Step {
                action: "pyroporch.api-call/request".into(),
                settings: serde_json::json!({"url": "https://api.other.example/", "auth": "bearer token", "secret": "{{secret:token}}"}).as_object().unwrap().clone(),
                delay_ms: 0,
            }],
            ..Key::default()
        };
        let report = engine.run_key(&key, Trigger::Press, "test", true);
        assert!(!report.ok);
        assert!(report.message.contains("isn't allowed to go to"), "{}", report.message);
    }

    /// Run with --nocapture to see how long a frame takes.
    #[test]
    fn every_animation_is_quick_enough_to_play_smoothly() {
        let (_dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let engine = engine_for(&ws);
        for animation in ["Breathing", "Rainbow: side to side", "Snake", "Campfire", "Digital rain", "Twinkling stars", "Game of Life"] {
            let settings = serde_json::json!({"animation": animation}).as_object().unwrap().clone();
            let started = std::time::Instant::now();
            for frame in 0..10u64 {
                let request = PaintRequest { settings: settings.clone(), rows: 4, cols: 8, blocks_per_key: 12, millis: frame * 66, restart: frame == 0 };
                engine.paint("pyroporch.screensaver", &request).unwrap();
            }
            let each = started.elapsed() / 10;
            eprintln!("{animation}: {each:?} a frame");
            assert!(each < std::time::Duration::from_millis(250), "{animation} took {each:?}");
        }
    }

    #[test]
    fn the_screen_saver_sample_plays_from_the_sandbox() {
        let (_dir, mut ws) = workspace();
        sync(&mut ws).unwrap();
        let engine = engine_for(&ws);
        let settings = serde_json::json!({"animation": "Snake", "start_now": true}).as_object().unwrap().clone();

        let key = Key {
            steps: vec![Step { action: "pyroporch.screensaver/toggle".into(), settings: settings.clone(), delay_ms: 0 }],
            ..Key::default()
        };
        let report = engine.run_key(&key, Trigger::Press, "test", true);
        assert!(report.ok, "{}", report.message);
        assert_eq!(report.deck.len(), 1, "it's allowed to ask, because its plugin.toml says so");

        let request = |millis, restart| PaintRequest { settings: settings.clone(), rows: 4, cols: 8, blocks_per_key: 6, millis, restart };
        let first = engine.paint("pyroporch.screensaver", &request(100, true)).unwrap();
        let later = engine.paint("pyroporch.screensaver", &request(3000, false)).unwrap();
        assert_eq!((first.rows, first.cols, first.blocks), (4, 8, 6));
        assert_eq!(first.rgb.len(), 48 * 24 * 3);
        assert_ne!(first.rgb, later.rgb, "the snake moved");
    }
}
