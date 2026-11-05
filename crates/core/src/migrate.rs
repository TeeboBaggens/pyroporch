//! Moves settings across from when the app had an earlier name.

use std::path::Path;

use crate::config::{CONFIG_VERSION, DEFAULT_ACCENT, Key, Profile, Settings};
use crate::paths::write_private;
use crate::policy::Grants;

const OLD_PREFIX: &str = "leandeck.";
const NEW_PREFIX: &str = "pyroporch.";
const OLD_DEFAULT_ACCENT: &str = "#5B8DEF";

fn new_id(id: &str) -> Option<String> {
    id.strip_prefix(OLD_PREFIX).map(|rest| format!("{NEW_PREFIX}{rest}"))
}

/// Renames a key's actions. Approval covers the action names, so a key the person had approved
/// is approved again under the new names, and nothing else gets approved.
fn update_key(key: &mut Key) {
    let was_approved = key.is_approved();
    for step in &mut key.steps {
        if let Some(action) = new_id(&step.action) {
            step.action = action;
        }
    }
    if was_approved {
        key.approve();
    }
}

fn update_profile(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut profile: Profile = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    // A key saved as a macro keeps its wait in the old form, which the file doesn't hold on
    // saving, so it is taken across before the action names change. The approval is tied to
    // those names, so it has to be read before they change too.
    profile.upgrade_old_fields();
    for page in &mut profile.pages {
        page.for_each_key_mut(&mut update_key);
    }
    profile.version = CONFIG_VERSION;
    let text = crate::config::to_toml(&profile)?;
    write_private(path, text.as_bytes()).map_err(|e| e.to_string())
}

fn update_settings(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut settings: Settings = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    if settings.ui.accent == OLD_DEFAULT_ACCENT {
        settings.ui.accent = DEFAULT_ACCENT.into();
    }
    for preset in &mut settings.presets {
        if let Some(action) = new_id(&preset.action) {
            preset.action = action;
        }
    }
    let text = crate::config::to_toml(&settings)?;
    write_private(path, text.as_bytes()).map_err(|e| e.to_string())
}

fn update_grants(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut grants: Grants = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    grants.plugins = std::mem::take(&mut grants.plugins)
        .into_iter()
        .map(|(id, grant)| (new_id(&id).unwrap_or(id), grant))
        .collect();
    let text = toml::to_string_pretty(&grants).map_err(|e| e.to_string())?;
    write_private(path, text.as_bytes()).map_err(|e| e.to_string())
}

/// Moves the old folder to the new name once, then updates what's inside. Files that can't be
/// read are left as they are: the app reports those itself.
pub fn from_old_name(old: &Path, new: &Path) -> Result<(), String> {
    if new.exists() || !old.is_dir() {
        return Ok(());
    }
    std::fs::rename(old, new).map_err(|e| e.to_string())?;
    if let Ok(entries) = std::fs::read_dir(new.join("profiles")) {
        for entry in entries.flatten() {
            let _ = update_profile(&entry.path());
        }
    }
    let _ = update_settings(&new.join("config.toml"));
    let _ = update_grants(&new.join("grants.toml"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Page, Step};
    use crate::security::approval_hash;
    use serde_json::json;

    fn step(action: &str) -> Step {
        Step {
            action: action.into(),
            settings: json!({"program": "ls"}).as_object().unwrap().clone(),
            delay_ms: 0,
        }
    }

    #[test]
    fn moves_everything_and_keeps_only_real_approvals() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        std::fs::create_dir_all(old.join("profiles")).unwrap();

        let steps = vec![step("leandeck.desktop/run_command")];
        let approved = Key { row: 0, col: 0, approved: approval_hash(&steps), steps: steps.clone(), ..Key::default() };
        let unapproved = Key { row: 0, col: 1, steps, ..Key::default() };
        let profile = Profile {
            pages: vec![Page { name: "Main".into(), keys: vec![approved, unapproved] }],
            ..Profile::default()
        };
        std::fs::write(old.join("profiles/default.toml"), crate::config::to_toml(&profile).unwrap()).unwrap();
        std::fs::write(old.join("config.toml"), "[ui]\naccent = \"#5B8DEF\"\n").unwrap();
        std::fs::write(old.join("grants.toml"), "[plugins.\"leandeck.api-call\"]\nhttp_hosts = [\"a.example\"]\n").unwrap();
        std::fs::write(old.join("vault.json"), "untouched").unwrap();

        from_old_name(&old, &new).unwrap();

        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(new.join("vault.json")).unwrap(), "untouched");
        let moved: Profile = toml::from_str(&std::fs::read_to_string(new.join("profiles/default.toml")).unwrap()).unwrap();
        let keys = &moved.pages[0].keys;
        assert_eq!(keys[0].steps[0].action, "pyroporch.desktop/run_command");
        assert!(keys[0].is_approved(), "approved stays approved");
        assert!(!keys[1].is_approved(), "unapproved stays unapproved");
        let settings: Settings = toml::from_str(&std::fs::read_to_string(new.join("config.toml")).unwrap()).unwrap();
        assert_eq!(settings.ui.accent, DEFAULT_ACCENT);
        let grants = std::fs::read_to_string(new.join("grants.toml")).unwrap();
        assert!(grants.contains("pyroporch.api-call"), "{grants}");
    }

    #[test]
    fn does_nothing_once_the_new_folder_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        from_old_name(&old, &new).unwrap();
        assert!(old.exists());
    }
}
