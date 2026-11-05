//! Drives the real window: finds things by name and clicks and types, as a person would.

use super::crop::KEY_IMAGE_SIZE;
use super::deck::{Drag, drop_hint};
use super::*;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use pyroporch_core::paths::Paths;
use pyroporch_core::vault::{KdfParams, Vault};
use std::sync::Arc;

const FAST: KdfParams = KdfParams {
    memory_kib: 1024,
    passes: 1,
    lanes: 1,
};
const PASSPHRASE: &str = "correct horse battery";
const SECRET_VALUE: &str = "marker-value-that-must-stay-hidden";


fn harness(dir: &tempfile::TempDir) -> Harness<'static, App> {
    let paths = Paths::at(dir.path());
    let mut ws = Workspace::load(paths.clone()).unwrap();
    ws.settings.hardware.enabled = false;
    pyroporch_core::sample_plugin::sync(&mut ws).unwrap();
    let vault = Vault::with_kdf(paths.vault(), FAST).unwrap();
    let engine = Arc::new(Engine::new(paths, vault, vec![Arc::new(pyroporch_desktop::DesktopActions)]));
    let ctx = egui::Context::default();
    let app = App::new(&ctx, ws, engine, Launch::default());
    HarnessBuilder::default()
        .with_size(egui::vec2(1280.0, 900.0))
        .build_ui_state(
            |ui, app: &mut App| {
                let ctx = ui.ctx().clone();
                app.background(&ctx);
                theme::apply(&ctx, &app.ws.settings.ui);
                app.draw(ui);
            },
            app,
        )
}

fn settle(harness: &mut Harness<'_, App>) {
    for _ in 0..4 {
        harness.step();
    }
}

fn click(harness: &mut Harness<'_, App>, label: &str) {
    harness.get_by_label(label).click();
    settle(harness);
}

/// Everything saved under the app's folder, as one string.
fn all_files(dir: &std::path::Path) -> String {
    let mut text = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            text.push_str(&all_files(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    text
}

fn wait_for_save(harness: &mut Harness<'_, App>) {
    std::thread::sleep(SAVE_DELAY + Duration::from_millis(50));
    settle(harness);
}

fn type_into(harness: &mut Harness<'_, App>, placeholder_index: usize, text: &str) {
    let field = harness.get_all_by_role(Role::PasswordInput).nth(placeholder_index).expect("password field");
    field.focus();
    field.type_text(text);
    settle(harness);
}

#[test]
fn every_screen_opens() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    for (tab, expected) in [
        ("Plugins", "API Calls"),
        ("Vault", "Set up your vault"),
        ("AI Assistant", "Connect an assistant"),
        ("Settings", "Appearance"),
    ] {
        click(&mut harness, tab);
        harness.get_by_label(expected);
    }
}

#[test]
fn a_key_can_be_selected_renamed_and_the_change_reaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Website");
    assert_eq!(harness.state().selected, Some((0, 0)));

    let title = harness.get_by_role_and_label(Role::TextInput, "Title");
    title.focus();
    title.type_text(" Two");
    settle(&mut harness);
    wait_for_save(&mut harness);

    let saved = std::fs::read_to_string(dir.path().join("profiles/default.toml")).unwrap();
    assert!(saved.contains("Website Two"), "{saved}");
}

#[test]
fn use_mode_presses_keys_and_navigation_works() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Use keys");
    click(&mut harness, "More");
    for _ in 0..50 {
        settle(&mut harness);
        if harness.state().page == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(harness.state().page, 1, "the More key goes to the next page");
    // And the Back key on that page returns.
    click(&mut harness, "Back");
    for _ in 0..50 {
        settle(&mut harness);
        if harness.state().page == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(harness.state().page, 0);
}

#[test]
fn an_unapproved_site_is_shown_and_can_be_allowed_in_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "API Status");
    harness.get_by_label_contains("until you allow it");
    click(&mut harness, "Allow api.github.com");
    let grants = Grants::load(&Paths::at(dir.path()));
    assert!(grants.plugins["pyroporch.api-call"].http_hosts.contains(&"api.github.com".to_string()));
    assert!(harness.query_by_label_contains("until you allow it").is_none());
}

#[test]
fn a_secret_goes_through_the_vault_screen_and_is_never_saved_in_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Vault");
    type_into(&mut harness, 0, PASSPHRASE);
    type_into(&mut harness, 1, PASSPHRASE);
    click(&mut harness, "Create vault");
    harness.get_by_label("Your vault is ready and open.");

    click(&mut harness, "+ Add");
    let name = harness.get_by_role_and_label(Role::TextInput, "Name");
    name.focus();
    name.type_text("my_token");
    settle(&mut harness);
    let value = harness.get_by_role_and_label(Role::PasswordInput, "Value");
    value.focus();
    value.type_text(SECRET_VALUE);
    settle(&mut harness);
    let hosts = harness.get_by_role_and_label(Role::TextInput, "Only send to");
    hosts.focus();
    hosts.type_text("api.example.com");
    settle(&mut harness);
    click(&mut harness, "Encrypt and store");

    harness.get_by_label("🔒 my_token");
    harness.get_by_label("only goes to api.example.com");
    wait_for_save(&mut harness);

    let everything = all_files(dir.path());
    assert!(everything.contains("my_token"), "the vault file should exist");
    assert!(!everything.contains(SECRET_VALUE), "value leaked to disk");
    assert!(!everything.contains(PASSPHRASE), "passphrase leaked to disk");

    // Locking shuts it, and the wrong passphrase doesn't open it.
    click(&mut harness, "🔒 Lock now");
    harness.get_by_label("The vault is locked");
    type_into(&mut harness, 0, "not the passphrase");
    click(&mut harness, "Unlock");
    harness.get_by_label("wrong passphrase");
}

#[test]
fn a_typed_credential_stops_the_key_from_being_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    // Put a plain token in the Sign-off key's text through the settings form.
    click(&mut harness, "Website");
    let address = harness.get_by_role_and_label(Role::TextInput, "Address");
    address.focus();
    address.type_text("?api_key=abc123");
    settle(&mut harness);
    harness.get_by_label_contains("Not saved");
    wait_for_save(&mut harness);
    let saved = std::fs::read_to_string(dir.path().join("profiles/default.toml")).unwrap();
    assert!(!saved.contains("abc123"), "{saved}");
}

fn preset(action: &str, settings: serde_json::Value) -> pyroporch_core::config::Preset {
    pyroporch_core::config::Preset {
        name: "From a file".into(),
        action: action.into(),
        settings: settings.as_object().unwrap().clone(),
        ..Default::default()
    }
}

#[test]
fn dropping_an_action_makes_a_key_and_on_a_key_adds_a_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let app = harness.state_mut();
    app.handle_drop(2, 2, &Drag::Action("pyroporch.desktop/hotkey".into()));
    let key = app.current_page().unwrap().key_at(2, 2).unwrap().clone();
    assert_eq!(key.title, "Hotkey");
    assert!(key.is_approved(), "the person chose this themselves");

    app.handle_drop(2, 2, &Drag::Action("pyroporch.desktop/media".into()));
    let key = app.current_page().unwrap().key_at(2, 2).unwrap().clone();
    assert_eq!(key.steps.len(), 2);
}

#[test]
fn dragging_keys_moves_or_swaps_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let app = harness.state_mut();
    app.handle_drop(2, 2, &Drag::Key(0, 0));
    let page = app.current_page().unwrap();
    assert_eq!(page.key_at(2, 2).unwrap().title, "Website");
    assert!(page.key_at(0, 0).is_none());

    app.handle_drop(0, 1, &Drag::Key(2, 2));
    let page = app.current_page().unwrap();
    assert_eq!(page.key_at(0, 1).unwrap().title, "Website");
    assert_eq!(page.key_at(2, 2).unwrap().title, "API Status");
}

#[test]
fn a_preset_from_a_file_never_arrives_approved() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let app = harness.state_mut();
    let risky = preset("pyroporch.desktop/run_command", serde_json::json!({"program": "/bin/sh"}));
    // Onto an empty slot, and onto a key that was already approved.
    app.handle_drop(2, 2, &Drag::Preset(risky.clone()));
    app.handle_drop(0, 2, &Drag::Preset(risky));
    for (row, col) in [(2, 2), (0, 2)] {
        let key = app.current_page().unwrap().key_at(row, col).unwrap().clone();
        assert!(!key.is_approved(), "key at {row},{col}");
        let report = app.engine.run_key(&key, Trigger::Press, "test", true);
        assert!(!report.ok);
    }
}

fn wait_for_page(harness: &mut Harness<'_, App>, page: usize) {
    for _ in 0..50 {
        settle(harness);
        if harness.state().page == page {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_folder_is_its_own_screen_with_a_way_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Use keys");
    click(&mut harness, "Media");
    assert_eq!(harness.state().folder, vec![(1, 4)]);
    assert_eq!(harness.state().page, 0, "still the same page");
    harness.get_by_label("Mute");
    assert!(harness.query_by_label("Website").is_none(), "the page's keys are gone");

    click(&mut harness, "Back");
    assert!(harness.state().folder.is_empty());
    harness.get_by_label("Website");
}

#[test]
fn page_keys_leave_folders_and_folders_stay_separate_from_pages() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Use keys");
    click(&mut harness, "More");
    wait_for_page(&mut harness, 1);
    assert_eq!(harness.state().page, 1);
    assert!(harness.state().folder.is_empty());
    harness.get_by_label("Second");
}

#[test]
fn folders_can_be_made_filled_and_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    {
        let app = harness.state_mut();
        app.handle_drop(2, 2, &Drag::Folder);
        assert!(app.current_page().unwrap().key_at(2, 2).unwrap().folder.is_some());

        // An action dropped on it lands inside; the back slot stays free.
        app.handle_drop(2, 2, &Drag::Action("pyroporch.desktop/hotkey".into()));
        // So does an existing key.
        app.handle_drop(2, 2, &Drag::Key(0, 0));
        assert!(app.current_page().unwrap().key_at(0, 0).is_none());
        let folder = app.current_page().unwrap().key_at(2, 2).unwrap().folder.clone().unwrap();
        assert_eq!(folder.keys.len(), 2);
        assert!(folder.keys.iter().all(|key| (key.row, key.col) != (0, 0)));

        // Nothing can land on the back key.
        app.open_folder((2, 2));
        app.handle_drop(0, 0, &Drag::Action("pyroporch.desktop/hotkey".into()));
        assert!(app.current_page().unwrap().key_at(0, 0).is_none());
        app.leave_folder();
        // A folder won't overwrite a key.
        app.handle_drop(0, 1, &Drag::Folder);
        assert!(app.current_page().unwrap().key_at(0, 1).unwrap().folder.is_none());
    }
    harness.state_mut().mark_profile_dirty();
    wait_for_save(&mut harness);
    let saved = std::fs::read_to_string(dir.path().join("profiles/default.toml")).unwrap();
    assert!(saved.contains("[pages.keys.folder]") || saved.contains("pages.keys.folder"), "{saved}");
}

#[test]
fn a_big_picture_opens_the_fitting_screen_and_ends_up_on_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let picture = dir.path().join("wide.png");
    image::RgbaImage::from_pixel(600, 300, image::Rgba([200, 30, 30, 255])).save(&picture).unwrap();

    harness.state_mut().use_picture(&picture, (0, 1));
    settle(&mut harness);
    assert!(harness.state().crop.is_some());
    harness.get_by_label("Fit the picture to the key");
    click(&mut harness, "Use this picture");
    assert!(harness.state().crop.is_none());

    let key = harness.state().current_page().unwrap().key_at(0, 1).unwrap().clone();
    let stored = Paths::at(dir.path()).image(&key.background).unwrap();
    let saved = image::open(stored).unwrap();
    assert_eq!((saved.width(), saved.height()), (KEY_IMAGE_SIZE, KEY_IMAGE_SIZE));
    assert!(key.is_approved(), "a picture doesn't change what a key does");
}

#[test]
fn a_small_square_picture_goes_straight_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let picture = dir.path().join("small.png");
    image::RgbaImage::from_pixel(96, 96, image::Rgba([30, 200, 30, 255])).save(&picture).unwrap();
    harness.state_mut().use_picture(&picture, (0, 1));
    settle(&mut harness);
    assert!(harness.state().crop.is_none());
    let key = harness.state().current_page().unwrap().key_at(0, 1).unwrap().clone();
    assert!(!key.background.is_empty());
    harness.step();
}

#[test]
fn hotkey_recording_works_in_the_real_form() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.state_mut().handle_drop(2, 2, &Drag::Action("pyroporch.desktop/hotkey".into()));
    settle(&mut harness);
    click(&mut harness, "⏺ Record");
    harness.get_by_label("Press keys now…");
    harness.key_press_modifiers(egui::Modifiers { ctrl: true, shift: true, ..egui::Modifiers::NONE }, egui::Key::F13);
    settle(&mut harness);
    let key = harness.state().current_page().unwrap().key_at(2, 2).unwrap().clone();
    assert_eq!(key.steps[0].settings["keys"], "ctrl+shift+f13");
    assert!(key.is_approved(), "the person recorded it themselves");
}

#[test]
fn labels_cannot_be_selected() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.ctx.global_style_mut(|style| assert!(!style.interaction.selectable_labels));
}

#[test]
fn export_and_import_work_from_the_settings_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let file = dir.path().join("backup.pyroporch");
    harness.state_mut().export_to(&file, false);
    assert!(harness.state().transfer_message.starts_with("Exported"));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("pyroporch-config"));

    harness.state_mut().import_from(&file);
    settle(&mut harness);
    harness.get_by_label("Import this file?");
    assert_eq!(harness.state().ws.profiles.len(), 1, "nothing changes before you say so");
    click(&mut harness, "Import");
    assert_eq!(harness.state().ws.profiles.len(), 2);
    assert!(harness.state().transfer_message.contains("Imported 1 profile"), "{}", harness.state().transfer_message);

    let junk = dir.path().join("junk.pyroporch");
    std::fs::write(&junk, "nonsense").unwrap();
    harness.state_mut().import_from(&junk);
    assert!(harness.state().import.is_none());
    assert!(harness.state().transfer_message.starts_with("Couldn't import"));
}

#[test]
fn the_running_option_is_on_by_default_and_on_the_settings_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    assert!(harness.state().ws.settings.run_in_background);
    click(&mut harness, "Settings");
    harness.get_by_label("Keep running in the background when the window is closed");
    harness.get_by_label("Quit Pyro Porch");
}

/// Clicks a key by name, when the action list also has something called that.
fn click_key(harness: &mut Harness<'_, App>, name: &str) {
    harness.get_by_role_and_label(Role::Button, name).click();
    settle(harness);
}

fn press_until(harness: &mut Harness<'_, App>, done: impl Fn(&App) -> bool) -> bool {
    for _ in 0..80 {
        settle(harness);
        if done(harness.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn the_sleep_key_turns_everything_off_and_any_press_wakes_without_running_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.state_mut().page = 1;
    settle(&mut harness);
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Sleep");
    assert!(press_until(&mut harness, |app| app.asleep), "the key puts the deck to sleep");
    assert!(harness.query_by_role_and_label(Role::Button, "Dimmer").is_none(), "no keys show while asleep");
    harness.get_by_label("Wake the deck");

    // Pressing where the Back key sits wakes the deck, and doesn't go back a page.
    click(&mut harness, "Sleeping key 2 0");
    assert!(!harness.state().asleep);
    press_until(&mut harness, |app| app.page != 1);
    assert_eq!(harness.state().page, 1, "that press only woke it");
    harness.get_by_role_and_label(Role::Button, "Dimmer");
}

#[test]
fn brightness_keys_change_and_save_the_brightness() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.state_mut().page = 1;
    settle(&mut harness);
    let start = harness.state().ws.settings.hardware.brightness;
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Brighter");
    assert!(press_until(&mut harness, |app| app.ws.settings.hardware.brightness == start + 10));
    click_key(&mut harness, "Dimmer");
    assert!(press_until(&mut harness, |app| app.ws.settings.hardware.brightness == start));
    harness.state_mut().control_deck(DeckControl::SetBrightness(35));
    wait_for_save(&mut harness);
    let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("brightness = 35"), "{saved}");
}

#[test]
fn the_action_list_offers_deck_controls_and_not_wait() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    for name in ["Sleep", "Set Brightness"] {
        assert!(harness.query_all_by_label(name).next().is_some(), "{name}");
    }
    assert!(harness.query_by_label("Wait").is_none());
}

/// Drags from one place to another with the pointer, as a person would.
fn drag(harness: &mut Harness<'_, App>, from: egui::Pos2, to: egui::Pos2) {
    harness.hover_at(from);
    settle(harness);
    harness.drag_at(from);
    settle(harness);
    harness.hover_at(to);
    settle(harness);
    harness.drop_at(to);
    settle(harness);
}

fn key_rect(harness: &Harness<'_, App>, name: &str) -> egui::Pos2 {
    harness.get_by_role_and_label(Role::Button, name).rect().center()
}

fn actions_on(harness: &Harness<'_, App>, row: u32, col: u32) -> Vec<String> {
    let key = harness.state().current_page().unwrap().key_at(row, col).unwrap();
    key.steps.iter().map(|step| step.action.clone()).collect()
}

#[test]
fn dragging_an_action_onto_a_key_adds_it_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let from = harness.get_by_label("Dimmer").rect().center();
    let to = key_rect(&harness, "New Tab");
    drag(&mut harness, from, to);
    assert_eq!(actions_on(&harness, 1, 0), vec!["pyroporch.desktop/hotkey".to_string(), "core.deck/dimmer".to_string()]);
    assert_eq!(harness.state().selected, Some((1, 0)), "the key opens, showing its actions");
    let line = &harness.state().activity.front().unwrap().text;
    assert!(line.contains("added Dimmer to New Tab, which now does 2 things"), "{line}");
    harness.get_by_label("Wait first");
    harness.get_by_label("Runs these in order, and stops if one fails.");
    assert!(harness.state().current_page().unwrap().key_at(1, 0).unwrap().is_approved());
}

#[test]
fn dragging_an_action_onto_an_empty_key_makes_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let from = harness.get_by_label("Dimmer").rect().center();
    let to = key_rect(&harness, "Empty key 2 2");
    drag(&mut harness, from, to);
    assert_eq!(harness.state().current_page().unwrap().key_at(2, 2).unwrap().title, "Dimmer");
}

#[test]
fn dragging_an_action_into_the_drop_area_adds_it_to_the_selected_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "New Tab");
    let from = harness.get_by_label("Dimmer").rect().center();
    let to = harness.get_by_label("Drop another action here to add it to this key").rect().center();
    drag(&mut harness, from, to);
    assert_eq!(actions_on(&harness, 1, 0).len(), 2);
    assert!(harness.state().current_page().unwrap().key_at(1, 0).unwrap().is_approved());
}

#[test]
fn a_folder_cannot_be_dropped_into_a_keys_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "New Tab");
    let from = harness.get_by_role_and_label(Role::Label, "Folder").rect().center();
    let to = harness.get_by_label("Drop another action here to add it to this key").rect().center();
    drag(&mut harness, from, to);
    assert_eq!(actions_on(&harness, 1, 0).len(), 1);
    assert!(harness.state().activity.front().unwrap().text.contains("only actions"));
}

#[test]
fn the_add_menu_puts_another_action_on_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "New Tab");
    click(&mut harness, "+ Add another action");
    harness.get_by_role_and_label(Role::Button, "Brighter").click();
    settle(&mut harness);
    assert_eq!(actions_on(&harness, 1, 0).last().unwrap(), "core.deck/brighter");
}

#[test]
fn each_action_after_the_first_can_set_its_own_wait() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Work Mode");
    assert_eq!(harness.get_all_by_label("Wait first").count(), 1, "one for the second action, none for the first");
    let key = harness.state().current_page().unwrap().key_at(1, 2).unwrap().clone();
    assert_eq!((key.steps[0].delay_ms, key.steps[1].delay_ms), (0, 500));
    assert!(key.is_approved());
}

#[test]
fn a_drop_says_what_it_will_do() {
    let action = Drag::Action("x/y".into());
    let slot = (1, 1);
    assert_eq!(drop_hint(&action, slot, true, false), Some("+ Add to this key"));
    assert_eq!(drop_hint(&action, slot, false, false), Some("New key"));
    assert_eq!(drop_hint(&action, slot, true, true), Some("Put in folder"));
    assert_eq!(drop_hint(&Drag::Key(0, 0), slot, true, false), Some("Swap keys"));
    assert_eq!(drop_hint(&Drag::Key(0, 0), slot, false, false), Some("Move here"));
    assert_eq!(drop_hint(&Drag::Key(1, 1), slot, true, false), None, "onto itself");
    assert_eq!(drop_hint(&Drag::Folder, slot, true, false), None, "a folder needs an empty key");
    assert_eq!(drop_hint(&Drag::Folder, slot, false, false), Some("New folder"));
}

#[test]
fn a_keys_actions_take_effect_as_they_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    {
        let app = harness.state_mut();
        app.handle_drop(2, 2, &Drag::Action("core.navigation/next_page".into()));
        app.handle_drop(2, 2, &Drag::Action("core.deck/brighter".into()));
        let key = app.current_page_mut().unwrap().key_at_mut(2, 2).unwrap();
        key.steps[1].delay_ms = 400;
        key.approve();
    }
    let start = harness.state().ws.settings.hardware.brightness;
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Next Page");
    assert!(press_until(&mut harness, |app| app.page == 1), "the page turns first");
    assert_eq!(harness.state().ws.settings.hardware.brightness, start, "the second action is still waiting");
    assert!(press_until(&mut harness, |app| app.ws.settings.hardware.brightness == start + 10));
}

#[test]
fn a_risky_preset_added_to_a_key_voids_its_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    let app = harness.state_mut();
    app.handle_drop(2, 2, &Drag::Action("pyroporch.desktop/hotkey".into()));
    assert!(app.current_page().unwrap().key_at(2, 2).unwrap().is_approved());
    let risky = preset("pyroporch.desktop/run_command", serde_json::json!({"program": "/bin/sh"}));
    app.handle_drop(2, 2, &Drag::Preset(risky));
    let key = app.current_page().unwrap().key_at(2, 2).unwrap().clone();
    assert_eq!(key.steps.len(), 2);
    assert!(!key.is_approved());
    assert!(!app.engine.run_key(&key, Trigger::Press, "test", true).ok);
}

#[test]
fn macro_is_gone_from_the_action_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    assert!(harness.query_by_label("Macro").is_none());
}

/// Sets up a screen saver key on an empty slot.
fn saver_key(harness: &mut Harness<'_, App>, settings: serde_json::Value) {
    let app = harness.state_mut();
    app.handle_drop(2, 2, &Drag::Action("pyroporch.screensaver/toggle".into()));
    let key = app.current_page_mut().unwrap().key_at_mut(2, 2).unwrap();
    key.steps[0].settings = settings.as_object().unwrap().clone();
    settle(harness);
}

#[test]
fn start_right_away_plays_at_once_and_any_key_stops_it_without_running() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    saver_key(&mut harness, serde_json::json!({"animation": "Snake", "start_now": true, "wait_minutes": 5}));
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Screen Saver");
    assert!(press_until(&mut harness, |app| app.saver.playing.is_some()));
    assert!(harness.query_by_role_and_label(Role::Button, "More").is_none(), "keys are hidden while it plays");
    harness.get_by_label("Stop the screen saver");

    // Pressing where the More key sits stops it, and doesn't change the page.
    click(&mut harness, "Sleeping key 2 4");
    assert!(harness.state().saver.playing.is_none());
    press_until(&mut harness, |app| app.page != 0);
    assert_eq!(harness.state().page, 0, "that press only stopped it");

    // It's still on, and comes back after the wait.
    assert!(harness.state().saver.armed.is_some());
    harness.get_by_role_and_label(Role::Button, "Screen Saver: On");
}

#[test]
fn pressing_again_turns_it_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    saver_key(&mut harness, serde_json::json!({"wait_minutes": 5, "start_now": false}));
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Screen Saver");
    assert!(press_until(&mut harness, |app| app.saver.armed.is_some()));
    assert!(harness.state().saver.playing.is_none(), "this one waits");
    click_key(&mut harness, "Screen Saver: On");
    assert!(press_until(&mut harness, |app| app.saver.armed.is_none()));
    harness.get_by_role_and_label(Role::Button, "Screen Saver");
}

#[test]
fn it_waits_for_the_quiet_spell_and_a_press_starts_the_wait_over() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    saver_key(&mut harness, serde_json::json!({"wait_minutes": 5, "start_now": false, "animation": "Campfire"}));
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Screen Saver");
    assert!(press_until(&mut harness, |app| app.saver.armed.is_some()));

    let minutes = |n: u64| Duration::from_secs(n * 60);
    harness.state_mut().saver.last_press = Instant::now() - minutes(4);
    settle(&mut harness);
    assert!(harness.state().saver.playing.is_none(), "not yet");

    // Another key press starts the wait over.
    harness.state_mut().saver.last_press = Instant::now() - minutes(6);
    click_key(&mut harness, "Website");
    settle(&mut harness);
    assert!(harness.state().saver.playing.is_none(), "that press counted");

    harness.state_mut().saver.last_press = Instant::now() - minutes(6);
    settle(&mut harness);
    assert!(harness.state().saver.playing.is_some(), "the wait is over");
    assert_eq!(harness.state().saver.armed.as_ref().unwrap().settings["animation"], "Campfire");
}

#[test]
fn with_no_wait_it_plays_once_and_turns_itself_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    saver_key(&mut harness, serde_json::json!({"wait_minutes": 0, "start_now": false}));
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Screen Saver");
    assert!(press_until(&mut harness, |app| app.saver.playing.is_some()), "nothing to wait for, so it starts");
    click(&mut harness, "Stop the screen saver");
    assert!(harness.state().saver.playing.is_none());
    assert!(harness.state().saver.armed.is_none());
}

#[test]
fn it_does_not_start_while_the_deck_is_asleep() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    saver_key(&mut harness, serde_json::json!({"wait_minutes": 1, "start_now": false}));
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Screen Saver");
    assert!(press_until(&mut harness, |app| app.saver.armed.is_some()));
    let app = harness.state_mut();
    app.asleep = true;
    app.saver.last_press = Instant::now() - Duration::from_secs(600);
    settle(&mut harness);
    assert!(harness.state().saver.playing.is_none());
}

#[test]
fn a_preview_plays_without_turning_it_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    saver_key(&mut harness, serde_json::json!({"wait_minutes": 5, "animation": "Digital rain"}));
    click(&mut harness, "Preview the animation");
    assert!(harness.state().saver.playing.is_some());
    assert!(harness.state().saver.armed.is_none());
    click(&mut harness, "Stop the screen saver");
    assert!(harness.state().saver.playing.is_none());
    assert!(harness.state().saver.armed.is_none());
}

#[test]
fn the_form_offers_the_options() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.state_mut().handle_drop(2, 2, &Drag::Action("pyroporch.screensaver/toggle".into()));
    settle(&mut harness);
    harness.get_by_label("Start right away");
    harness.get_by_label("Start after no key presses for (minutes)");
    harness.get_by_label("Animation");
    harness.get_by_label("Speed");
    let key = harness.state().current_page().unwrap().key_at(2, 2).unwrap().clone();
    assert!(key.is_approved(), "it only touches the deck, so it needs no approval");
    assert!(harness.state().engine.check_key(&key).is_empty());
}

#[test]
fn every_animation_draws_frames_from_the_sandboxed_plugin() {
    for animation in ["Breathing", "Rainbow: side to side", "Rainbow: up and down", "Snake", "Campfire", "Digital rain", "Twinkling stars", "Game of Life"] {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = harness(&dir);
        settle(&mut harness);
        harness.state_mut().start_on(&format!("saver:{animation}"));
        assert!(press_until(&mut harness, |app| app.saver.playing.as_ref().is_some_and(|playing| playing.frame.is_some())), "{animation}");
    }
}

#[test]
fn removing_the_screen_saver_while_it_plays_stops_it_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.state_mut().start_on("saver:Snake");
    assert!(press_until(&mut harness, |app| app.saver.playing.as_ref().is_some_and(|playing| playing.frame.is_some())));
    harness.state_mut().remove_plugin("pyroporch.screensaver");
    assert!(press_until(&mut harness, |app| app.saver.playing.is_none()));
    assert!(harness.state().activity.front().unwrap().text.contains("the screen saver stopped"));
}

fn plugin_folder(dir: &tempfile::TempDir) -> std::path::PathBuf {
    Paths::at(dir.path()).plugins_dir()
}

#[test]
fn the_plugins_screen_explains_what_is_built_in_and_where_plugins_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Plugins");
    harness.get_by_label("Built into the app");
    harness.get_by_label("From your plugins folder");
    harness.get_by_label("Desktop");
    harness.get_by_label("API Calls");
    assert_eq!(harness.get_all_by_label("sample, on, in sandbox").count(), 2);
    harness.get_by_label("Open the plugins folder");
    harness.get_by_label("Refresh");
}

#[test]
fn refresh_finds_a_folder_added_while_the_app_is_open_and_says_why_it_is_not_a_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Plugins");

    // What happened when the screen saver's source folder was copied in by hand.
    let copied = plugin_folder(&dir).join("copied-source");
    std::fs::create_dir_all(copied.join("src")).unwrap();
    std::fs::write(copied.join("Cargo.toml"), "[package]\nname = \"pyroporch-desktop\"\n").unwrap();
    assert!(harness.query_by_label("copied-source").is_none());

    click(&mut harness, "Refresh");
    harness.get_by_label("copied-source");
    harness.get_by_label("not a plugin yet");
    harness.get_by_label_contains("already built into the app");
    assert!(harness.query_by_label("Approve and turn on").is_none(), "nothing to approve");
    assert!(harness.state().plugin_message.contains("Found 3 item"), "{}", harness.state().plugin_message);

    // And taking it out again clears it.
    std::fs::remove_dir_all(&copied).unwrap();
    click(&mut harness, "Refresh");
    assert!(harness.query_by_label("copied-source").is_none());
    assert!(harness.state().plugin_message.contains("Found 2 item"), "{}", harness.state().plugin_message);
}

#[test]
fn the_samples_are_in_the_plugins_folder_and_can_be_removed_and_restored() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    for folder in ["api-call", "screensaver"] {
        assert!(plugin_folder(&dir).join(folder).join("plugin.toml").exists());
        assert!(plugin_folder(&dir).join(folder).join("source/lib.rs").exists(), "so people can see how it's made");
    }
    harness.get_by_label("API Request");

    click(&mut harness, "Plugins");
    assert!(harness.query_by_label("Restore the sample plugins").is_none(), "nothing to restore");

    // Removing takes a second click, so a slip doesn't lose anything.
    harness.get_all_by_label("Remove").next().unwrap().click();
    settle(&mut harness);
    assert!(plugin_folder(&dir).join("api-call").exists());
    click(&mut harness, "Click again to remove");
    assert!(!plugin_folder(&dir).join("api-call").exists());
    click_key(&mut harness, "Deck");
    assert!(harness.query_by_label("API Request").is_none(), "its action is gone from the list");
    // A key that used it says so, rather than failing quietly.
    harness.get_by_role_and_label(Role::Button, "API Status");
    let key = harness.state().current_page().unwrap().key_at(0, 1).unwrap().clone();
    assert!(!harness.state().engine.check_key(&key).is_empty());

    click(&mut harness, "Plugins");
    click(&mut harness, "Restore the sample plugins");
    assert!(plugin_folder(&dir).join("api-call/plugin.wasm").exists());
    click_key(&mut harness, "Deck");
    harness.get_by_label("API Request");
    assert!(harness.state().engine.check_key(&key).is_empty());
}

#[test]
fn a_built_in_module_has_no_remove_button_and_a_plugin_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Plugins");
    assert_eq!(harness.get_all_by_label("Remove").count(), 2, "only the ones in the folder");
    harness.get_all_by_label("Turn off").next().unwrap().click();
    settle(&mut harness);
    assert!(harness.state().engine.spec("pyroporch.api-call/request").is_none());
    click(&mut harness, "Approve and turn on");
    assert!(harness.state().engine.spec("pyroporch.api-call/request").is_some());
}

#[test]
fn the_menu_bar_icon_colour_is_a_setting_and_saves() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Settings");
    assert_eq!(harness.state().ws.settings.ui.tray_icon, "color");
    harness.get_by_label(if cfg!(target_os = "macos") { "Menu bar icon" } else { "Tray icon" });
    click(&mut harness, "White");
    assert_eq!(harness.state().ws.settings.ui.tray_icon, "white");
    wait_for_save(&mut harness);
    let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("tray_icon = \"white\""), "{saved}");
    click(&mut harness, "Black");
    assert_eq!(harness.state().ws.settings.ui.tray_icon, "black");
}

#[test]
fn the_banner_says_which_mode_it_is_and_what_clicking_does() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.get_by_label("EDIT MODE");
    harness.get_by_label_contains("right-click to remove it");
    assert!(harness.query_by_label("USE MODE").is_none());

    click(&mut harness, "Use keys");
    harness.get_by_label("USE MODE");
    harness.get_by_label_contains("Clicking a key runs it. To change or remove keys");
    assert!(harness.query_by_label("EDIT MODE").is_none());

    click(&mut harness, "Edit keys");
    harness.get_by_label("EDIT MODE");
}

#[test]
fn use_mode_takes_away_what_only_works_in_edit_mode_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Website");
    harness.get_by_label("Remove key");
    harness.get_by_label("Folder");
    harness.get_by_label("+ Profile");
    harness.get_by_label("Delete page");

    click(&mut harness, "Use keys");
    for gone in ["Remove key", "+ Profile", "Delete page", "Hotkey", "Title"] {
        assert!(harness.query_by_label(gone).is_none(), "{gone}");
    }
    harness.get_by_label("You're in Use mode");
    harness.get_by_label_contains("Switch to Edit to add actions");

    // The way back is right there, and the key is still selected.
    click(&mut harness, "Switch to Edit");
    assert!(!harness.state().use_mode);
    harness.get_by_label("Remove key");
    assert_eq!(harness.state().selected, Some((0, 0)));
}

#[test]
fn clicking_an_empty_key_in_use_mode_says_how_to_set_it_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Use keys");
    click_key(&mut harness, "Empty key 2 2");
    assert!(harness.state().activity.front().unwrap().text.contains("switch to Edit"));
    assert!(harness.state().current_page().unwrap().key_at(2, 2).is_none());
}

#[test]
fn keys_cannot_be_dragged_around_in_use_mode() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click(&mut harness, "Use keys");
    let from = key_rect(&harness, "New Tab");
    let to = key_rect(&harness, "Empty key 2 2");
    drag(&mut harness, from, to);
    let page = harness.state().current_page().unwrap();
    assert!(page.key_at(1, 0).is_some(), "it stayed where it was");
    assert!(page.key_at(2, 2).is_none());
}

#[test]
fn an_action_group_can_be_collapsed_and_stays_that_way() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    harness.get_by_label("Dimmer");
    harness.get_by_label("Next Page");

    click_key(&mut harness, "DECK");
    assert!(harness.query_by_label("Dimmer").is_none(), "folded away");
    assert!(harness.query_by_label("Set Brightness").is_none());
    harness.get_by_label("Next Page");
    harness.get_by_role_and_label(Role::Button, "DECK");

    // It's remembered for next time.
    wait_for_save(&mut harness);
    let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("core.deck"), "{saved}");
    drop(harness);
    let mut harness = super::tests::harness(&dir);
    settle(&mut harness);
    assert!(harness.query_by_label("Dimmer").is_none(), "still folded after a restart");

    click_key(&mut harness, "DECK");
    harness.get_by_label("Dimmer");
    assert!(harness.state().ws.settings.ui.collapsed_groups.is_empty());
}

#[test]
fn a_search_looks_inside_collapsed_groups() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    click_key(&mut harness, "DECK");
    assert!(harness.query_by_label("Dimmer").is_none());

    let search = harness.get_by_role_and_label(Role::TextInput, "Search actions");
    search.focus();
    search.type_text("dim");
    settle(&mut harness);
    harness.get_by_label("Dimmer");

    // Clicking the heading during a search doesn't change what's remembered.
    click_key(&mut harness, "DECK");
    assert_eq!(harness.state().ws.settings.ui.collapsed_groups, vec!["core.deck".to_string()]);
    harness.get_by_label("Dimmer");

    harness.state_mut().search.clear();
    settle(&mut harness);
    assert!(harness.query_by_label("Dimmer").is_none(), "folded again once the search is over");
}

#[test]
fn every_group_can_be_collapsed() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir);
    settle(&mut harness);
    for (heading, inside) in [
        ("ORGANIZE", "Folder"),
        ("NAVIGATION", "Next Page"),
        ("DECK", "Dimmer"),
        ("DESKTOP", "Open Website"),
        ("API CALLS", "API Request"),
        ("SCREEN SAVER", "Screen Saver"),
    ] {
        harness.get_by_role_and_label(Role::Label, inside);
        click_key(&mut harness, heading);
        assert!(harness.query_by_role_and_label(Role::Label, inside).is_none(), "{heading}");
    }
}
