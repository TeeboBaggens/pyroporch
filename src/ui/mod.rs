//! The window: app state, the top bar, background work and the screenshot option.

mod crop;
mod deck;
mod hotkey;
mod saver;
mod screens;
mod secret_field;
mod theme;

#[cfg(all(test, feature = "desktop-actions"))]
mod tests;


use eframe::egui::{self, Color32, RichText};
use pyroporch_core::config::{BACK_SLOT, Key, Page, Profile, Workspace};
use pyroporch_core::engine::{Effect, Engine, RunReport};
use pyroporch_core::paths::{audit, stamp};
use pyroporch_core::policy::{Grants, Pending};
use pyroporch_core::vault::VaultState;
use pyroporch_sdk::{DeckControl, Navigate, Trigger};
use secret_field::SecretBuffer;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{hardware, instance, tray};

const SCREENSHOT_FRAMES: u32 = 120;
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(90);
const SAVE_DELAY: Duration = Duration::from_millis(400);
const ACTIVITY_LINES: usize = 60;
/// The longest a key may ask to be refreshed after. A day is plenty, and it keeps the number
/// well inside what a clock can hold.
const MAX_REFRESH_SECONDS: f64 = 86_400.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Deck,
    Plugins,
    Vault,
    Assistant,
    Settings,
}

/// One key's place, across all profiles.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct KeyId {
    pub profile: String,
    pub page: usize,
    /// The folders opened to reach it, outermost first.
    pub folder: Vec<(u32, u32)>,
    pub row: u32,
    pub col: u32,
}

pub struct ActivityLine {
    pub ok: bool,
    pub text: String,
}

/// Where a new secret's reference should go once it is saved.
#[derive(Clone)]
pub struct SecretTarget {
    /// The key the secret was asked for, so it goes on that one and not on whatever is
    /// selected by the time it is saved.
    pub id: KeyId,
    pub step: usize,
    pub field: String,
}

/// The "add a secret" dialog.
#[derive(Default)]
pub struct SecretDialog {
    pub name: String,
    pub value: SecretBuffer,
    pub hosts: String,
    pub reason: String,
    pub target: Option<SecretTarget>,
    pub value_field: Option<egui::Id>,
    pub passphrase: SecretBuffer,
    pub error: String,
}

/// Fields on the vault screen.
#[derive(Default)]
pub struct VaultForm {
    pub passphrase: SecretBuffer,
    pub confirm: SecretBuffer,
    pub new_passphrase: SecretBuffer,
    pub message: String,
    pub is_error: bool,
}

/// How the app was started.
#[derive(Default)]
pub struct Launch {
    pub screenshot: Option<PathBuf>,
    /// Held by a normal start. Tests and screenshots go without, and without the tray too.
    pub lock: Option<instance::Lock>,
}

/// An export file that's been read, waiting for the person to say go.
pub struct ImportDialog {
    pub bundle: pyroporch_core::transfer::Bundle,
    pub use_appearance: bool,
}

struct Shot {
    path: PathBuf,
    frames: u32,
    requested: bool,
    done: Arc<Mutex<Option<Result<(), String>>>>,
}

pub struct App {
    pub ws: Workspace,
    pub engine: Arc<Engine>,
    pub view: View,
    pub page: usize,
    /// The folders opened on this page, outermost first. Empty on the page itself.
    pub folder: Vec<(u32, u32)>,
    pub selected: Option<(u32, u32)>,
    /// A key being edited that can't be saved yet, because it holds a plain credential.
    pub draft: Option<Key>,
    pub use_mode: bool,
    pub search: String,
    pub activity: VecDeque<ActivityLine>,
    pub live_titles: HashMap<KeyId, String>,
    pub flipped: HashSet<KeyId>,
    pub flashes: HashMap<KeyId, (Instant, bool)>,
    pub running: HashSet<KeyId>,
    pub secret_dialog: Option<SecretDialog>,
    pub vault_form: VaultForm,
    pub new_profile_name: Option<String>,
    pub new_page_name: Option<String>,
    pub show_requests: bool,
    pub pending: Pending,
    pub grants: Grants,
    /// When those two files were last read, so they aren't read for nothing.
    pending_stamp: Option<std::time::SystemTime>,
    grants_stamp: Option<std::time::SystemTime>,
    pub deck_status: Option<String>,
    /// The pictures drawn on keys, kept so they are read only once.
    pub textures: RefCell<HashMap<String, Option<egui::TextureHandle>>>,
    pub crop: Option<crop::CropDialog>,
    ctx_for_crop: egui::Context,
    pub import: Option<ImportDialog>,
    /// What the last export or import did, shown on the Settings screen.
    pub transfer_message: String,
    /// What the last read of the plugins folder found.
    pub plugin_message: String,
    /// A plugin waiting for a second click before it's removed.
    pub confirm_remove_plugin: Option<String>,
    /// The connected deck's grid, for offering to match it.
    pub deck_shape: Option<(u32, u32)>,
    /// Every key off, on the deck and on screen, until something wakes it.
    pub asleep: bool,
    last_asleep: bool,
    pub saver: saver::SaverControl,
    last_saver_playing: bool,
    last_tray_style: String,
    /// A folder key waiting for a second click before it and its contents go.
    pub confirm_remove: Option<(u32, u32)>,
    logo: Option<egui::TextureHandle>,
    _lock: Option<instance::Lock>,
    has_lock: bool,
    tray: Option<tray::Tray>,
    // Only read on macOS, where the Dock icon can bring a hidden window back.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    hidden: bool,
    #[cfg(target_os = "macos")]
    was_active: bool,
    /// The vault was open at the last look, to notice it closing.
    vault_was_open: bool,
    /// The profiles with unsaved changes, and when each was last edited. The profile is named,
    /// because the person may switch to another one before the change is saved.
    dirty_profiles: HashMap<String, Instant>,
    dirty_settings: Option<Instant>,
    done_tx: Sender<(KeyId, RunReport, Trigger)>,
    done_rx: Receiver<(KeyId, RunReport, Trigger)>,
    effect_tx: Sender<(KeyId, Effect)>,
    effect_rx: Receiver<(KeyId, Effect)>,
    refresh_due: HashMap<KeyId, Instant>,
    last_tick: Instant,
    last_plugin_scan: Instant,
    applied_theme: String,
    pub hardware: Option<hardware::Handle>,
    last_visuals: Vec<hardware::KeyVisual>,
    last_brightness: u8,
    shot: Option<Shot>,
}

impl App {
    pub fn new(egui_ctx: &egui::Context, mut ws: Workspace, engine: Arc<Engine>, launch: Launch) -> Self {
        let Launch { screenshot, lock } = launch;
        let (done_tx, done_rx) = channel();
        let (effect_tx, effect_rx) = channel();
        let has_lock = lock.is_some();
        let last_tray_style = ws.settings.ui.tray_icon.clone();
        let tray = has_lock.then(|| tray::Tray::new(egui_ctx, &last_tray_style)).and_then(|made| match made {
            Ok(tray) => Some(tray),
            Err(error) => {
                eprintln!("pyroporch: no menu bar item: {error}");
                None
            }
        });
        let ctx = egui_ctx.clone();
        let hardware_on = ws.settings.hardware.enabled && screenshot.is_none();
        let hardware = hardware_on.then(|| hardware::start(ws.settings.hardware.brightness, move || ctx.request_repaint()));
        let shot = screenshot.map(|path| {
            // If the window never gets to draw, fail rather than hang.
            std::thread::spawn(|| {
                std::thread::sleep(SCREENSHOT_TIMEOUT);
                eprintln!("pyroporch: the screenshot timed out");
                std::process::exit(1);
            });
            Shot {
                path,
                frames: 0,
                requested: false,
                done: Arc::new(Mutex::new(None)),
            }
        });
        let recovered = ws.recover_autosaves();
        let pending = Pending::load(&ws.paths);
        let grants = Grants::load(&ws.paths);
        let last_brightness = ws.settings.hardware.brightness;
        let mut app = Self {
            ws,
            engine,
            view: View::Deck,
            page: 0,
            folder: Vec::new(),
            selected: Some((0, 1)),
            draft: None,
            use_mode: false,
            search: String::new(),
            activity: VecDeque::new(),
            live_titles: HashMap::new(),
            flipped: HashSet::new(),
            flashes: HashMap::new(),
            running: HashSet::new(),
            secret_dialog: None,
            vault_form: VaultForm::default(),
            new_profile_name: None,
            new_page_name: None,
            show_requests: false,
            pending,
            grants,
            pending_stamp: None,
            grants_stamp: None,
            deck_status: None,
            textures: RefCell::new(HashMap::new()),
            crop: None,
            ctx_for_crop: egui_ctx.clone(),
            import: None,
            transfer_message: String::new(),
            plugin_message: String::new(),
            confirm_remove_plugin: None,
            deck_shape: None,
            asleep: false,
            last_asleep: false,
            saver: saver::SaverControl::default(),
            last_saver_playing: false,
            last_tray_style,
            confirm_remove: None,
            logo: None,
            _lock: lock,
            has_lock,
            tray,
            hidden: false,
            vault_was_open: false,
            #[cfg(target_os = "macos")]
            was_active: false,
            dirty_profiles: HashMap::new(),
            dirty_settings: None,
            done_tx,
            done_rx,
            effect_tx,
            effect_rx,
            refresh_due: HashMap::new(),
            last_tick: Instant::now(),
            last_plugin_scan: Instant::now(),
            applied_theme: String::new(),
            hardware,
            last_visuals: Vec::new(),
            last_brightness,
            shot,
        };
        app.apply_theme(egui_ctx);
        for id in recovered {
            app.log(true, format!("brought back unsaved changes to {id}. Press Save to keep them"));
        }
        app
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let ui = &self.ws.settings.ui;
        let signature = format!("{}|{}|{}", ui.theme, ui.accent, ui.scale);
        if signature != self.applied_theme {
            theme::apply(ctx, ui);
            self.applied_theme = signature;
        }
    }

    pub fn apply_theme_now(&mut self, ctx: &egui::Context) {
        self.apply_theme(ctx);
    }

    pub fn palette(&self) -> theme::Palette {
        theme::palette(&self.ws.settings.ui)
    }

    pub fn active_id(&self) -> String {
        self.ws.settings.active_profile.clone()
    }

    pub fn profile(&self) -> Option<&Profile> {
        self.ws.active()
    }

    pub fn profile_mut(&mut self) -> Option<&mut Profile> {
        let id = self.active_id();
        self.ws.profiles.get_mut(&id)
    }

    pub fn key_id(&self, row: u32, col: u32) -> KeyId {
        KeyId {
            profile: self.active_id(),
            page: self.page,
            folder: self.folder.clone(),
            row,
            col,
        }
    }

    pub fn in_folder(&self) -> bool {
        !self.folder.is_empty()
    }

    /// The screen a key lives on.
    fn screen_of(&self, id: &KeyId) -> Option<&Page> {
        self.ws.profiles.get(&id.profile)?.pages.get(id.page)?.descend(&id.folder)
    }

    /// The screen a key lives on, to change it.
    fn screen_of_mut(&mut self, id: &KeyId) -> Option<&mut Page> {
        let (page, folder) = (id.page, id.folder.clone());
        self.ws.profiles.get_mut(&id.profile)?.pages.get_mut(page)?.descend_mut(&folder)
    }

    pub fn open_folder(&mut self, slot: (u32, u32)) {
        self.folder.push(slot);
        self.selected = None;
        self.draft = None;
        self.confirm_remove = None;
    }

    /// Steps out of the folder, to the screen it's on.
    pub fn leave_folder(&mut self) {
        self.folder.pop();
        self.selected = None;
        self.draft = None;
        self.confirm_remove = None;
    }

    /// Goes back to the page itself.
    pub fn leave_all_folders(&mut self) {
        self.folder.clear();
        self.selected = None;
        self.draft = None;
        self.confirm_remove = None;
    }

    pub fn mark_profile_dirty(&mut self) {
        self.dirty_profiles.insert(self.active_id(), Instant::now());
        // A live title belongs to what the key was. The next run brings back a current one.
        self.live_titles.clear();
    }

    /// Reads the requests and allowances again, but only if their files have changed.
    fn reload_requests(&mut self) {
        let pending = stamp(&self.ws.paths.pending());
        if pending != self.pending_stamp {
            self.pending_stamp = pending;
            self.pending = Pending::load(&self.ws.paths);
        }
        let grants = stamp(&self.ws.paths.grants());
        if grants != self.grants_stamp {
            self.grants_stamp = grants;
            self.grants = Grants::load(&self.ws.paths);
        }
    }

    pub fn mark_settings_dirty(&mut self) {
        self.dirty_settings = Some(Instant::now());
    }

    pub fn log(&mut self, ok: bool, text: impl Into<String>) {
        self.activity.push_front(ActivityLine { ok, text: text.into() });
        self.activity.truncate(ACTIVITY_LINES);
    }

    /// Runs a key on its own thread, so a slow request can't freeze the window.
    pub fn press(&mut self, ctx: &egui::Context, id: KeyId, trigger: Trigger) {
        if self.running.contains(&id) {
            return;
        }
        let key = self.screen_of(&id).and_then(|screen| screen.key_at(id.row, id.col)).cloned();
        let Some(key) = key else {
            return;
        };
        if key.folder.is_some() {
            // A timer can't open a folder, and neither can a key on a screen that's gone.
            if trigger == Trigger::Press && id.folder == self.folder && id.page == self.page {
                self.open_folder((id.row, id.col));
            }
            return;
        }
        if trigger == Trigger::Press {
            self.saver.last_press = Instant::now();
        }
        self.running.insert(id.clone());
        let (engine, tx, ctx) = (self.engine.clone(), self.done_tx.clone(), ctx.clone());
        let effects = self.effect_tx.clone();
        std::thread::spawn(move || {
            let (wake, pressed) = (ctx.clone(), id.clone());
            let report = engine.run_key_live(&key, trigger, "app", true, |effect| {
                let _ = effects.send((pressed.clone(), effect));
                wake.request_repaint();
            });
            let _ = tx.send((id, report, trigger));
            ctx.request_repaint();
        });
    }

    fn finish_runs(&mut self, ctx: &egui::Context) {
        // Effects first: a key's own are all sent before it reports being done.
        while let Ok((id, effect)) = self.effect_rx.try_recv() {
            match effect {
                Effect::Navigate(to) => self.navigate(to),
                Effect::ScreenSaver { plugin, settings, timing } => self.toggle_saver(ctx, id, plugin, settings, timing),
                Effect::Deck(control) => self.control_deck(control),
            }
        }
        while let Ok((id, report, trigger)) = self.done_rx.try_recv() {
            self.running.remove(&id);
            self.flashes.insert(id.clone(), (Instant::now(), report.ok));
            if let Some(title) = report.title.clone() {
                self.live_titles.insert(id.clone(), title);
            }
            if report.ok && trigger == Trigger::Press {
                let toggles = self
                    .screen_of(&id)
                    .and_then(|screen| screen.key_at(id.row, id.col))
                    .is_some_and(|k| k.toggle);
                if toggles && !self.flipped.remove(&id) {
                    self.flipped.insert(id.clone());
                }
            }
            // A timer that keeps failing shouldn't flood the list with the same line.
            let repeat = self.activity.front().is_some_and(|l| l.text == report.message);
            if !report.message.is_empty() && !(trigger == Trigger::Refresh && repeat) {
                self.log(report.ok, report.message.clone());
            }
            self.reload_requests();
        }
    }

    /// Carries out a deck action. A brightness is saved, the same as moving the Settings slider.
    pub fn control_deck(&mut self, control: DeckControl) {
        if control == DeckControl::Sleep {
            self.asleep = true;
            return;
        }
        let current = self.ws.settings.hardware.brightness;
        if let Some(brightness) = control.brightness_after(current) {
            self.ws.settings.hardware.brightness = brightness;
            self.mark_settings_dirty();
            self.log(true, format!("brightness {brightness}%"));
        }
    }

    fn navigate(&mut self, to: Navigate) {
        let pages = self.profile().map_or(0, |p| p.pages.len());
        if pages == 0 {
            return;
        }
        match to {
            Navigate::NextPage => self.page = (self.page + 1) % pages,
            Navigate::PreviousPage => self.page = (self.page + pages - 1) % pages,
            Navigate::Page(name) => {
                let found = self.profile().and_then(|p| p.pages.iter().position(|page| page.name == name));
                match found {
                    Some(page) => self.page = page,
                    None => self.log(false, format!("there is no page called {name}")),
                }
            }
            Navigate::Profile(id) => {
                if self.ws.profiles.contains_key(&id) {
                    self.switch_profile(id);
                } else {
                    self.log(false, format!("there is no profile called {id}"));
                }
            }
        }
        self.leave_all_folders();
    }

    pub fn switch_profile(&mut self, id: String) {
        self.ws.settings.active_profile = id;
        self.page = 0;
        self.leave_all_folders();
        self.mark_settings_dirty();
    }

    /// Puts the current profile in its own file. Assistants see it from then on.
    pub fn save_active_profile(&mut self) {
        let id = self.active_id();
        self.dirty_profiles.remove(&id);
        match self.ws.save_profile(&id) {
            Ok(()) => self.log(true, format!("saved {id}")),
            Err(error) => self.log(false, format!("couldn't save: {error}")),
        }
    }

    /// Whether the current profile has changes that aren't in its own file.
    pub fn active_has_changes(&self) -> bool {
        let id = self.active_id();
        self.dirty_profiles.contains_key(&id) || self.ws.has_unsaved(&id)
    }

    /// Keeps what changed in an autosave file, a moment after the last edit. Settings go straight
    /// to their file, because switches such as assistant access have to count straight away.
    fn autosave(&mut self) {
        let due: Vec<String> = self
            .dirty_profiles
            .iter()
            .filter(|(_, at)| at.elapsed() >= SAVE_DELAY)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.dirty_profiles.remove(&id);
            if let Err(error) = self.ws.autosave_profile(&id) {
                self.log(false, format!("couldn't autosave: {error}"));
            }
        }
        if self.dirty_settings.is_some_and(|at| at.elapsed() >= SAVE_DELAY) {
            self.dirty_settings = None;
            if let Err(error) = self.ws.save_settings() {
                self.log(false, format!("couldn't save the settings: {error}"));
            }
        }
    }

    /// Once a second: pick up outside edits, requests and timers.
    fn tick(&mut self, ctx: &egui::Context) {
        if self.last_tick.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_tick = Instant::now();
        if self.dirty_profiles.is_empty() && self.dirty_settings.is_none() && self.ws.check_reload() {
            if let Some(pages) = self.profile().map(|p| p.pages.len())
                && self.page >= pages
            {
                self.page = 0;
            }
            // A folder can vanish in an outside edit.
            if self.current_page().is_none() {
                self.leave_all_folders();
            }
            self.engine.refresh_plugins_if_changed();
            self.apply_theme(ctx);
        }
        if self.has_lock {
            if instance::take_request(&self.ws.paths.show_request()) {
                self.show_window(ctx);
            }
            if instance::take_request(&self.ws.paths.quit_request()) {
                self.quit_now();
            }
        }
        self.reload_requests();
        self.clear_finished_flashes();
        let minutes = u64::from(self.ws.settings.security.auto_lock_minutes);
        let mut locked_now = false;
        let mut open_now = false;
        if let Ok(mut vault) = self.engine.vault.lock() {
            let _ = vault.reload();
            locked_now = vault.lock_if_idle(Duration::from_secs(minutes * 60));
            open_now = vault.state() == pyroporch_core::vault::VaultState::Unlocked;
        }
        if locked_now {
            self.log(true, "the vault locked itself after being idle");
        }
        // Anything typed and not sent must not be sitting there, one click from unlocking it again.
        if self.vault_was_open && !open_now {
            self.vault_form.passphrase.wipe();
            self.vault_form.confirm.wipe();
            self.vault_form.new_passphrase.wipe();
        }
        self.vault_was_open = open_now;
        if self.view == View::Plugins && self.last_plugin_scan.elapsed() >= Duration::from_secs(5) {
            self.last_plugin_scan = Instant::now();
            self.engine.refresh_plugins_if_changed();
        }
        self.run_timers(ctx);
    }

    /// Re-runs keys on the current page that asked to refresh on a timer.
    fn run_timers(&mut self, ctx: &egui::Context) {
        let Some(page) = self.current_page().cloned() else {
            return;
        };
        for key in &page.keys {
            let seconds = key
                .steps
                .iter()
                .filter(|s| self.engine.is_refreshable(&s.action))
                .filter_map(|s| s.settings.get("refresh_seconds").and_then(serde_json::Value::as_f64))
                .fold(0.0, f64::max);
            // The setting comes from a file, so it can be anything at all. A value too big for a
            // Duration, or too big to add to the clock, would bring the app down.
            if !(1.0..=MAX_REFRESH_SECONDS).contains(&seconds) {
                continue;
            }
            let id = self.key_id(key.row, key.col);
            let due = self.refresh_due.get(&id).is_none_or(|at| Instant::now() >= *at);
            if due {
                let Ok(wait) = Duration::try_from_secs_f64(seconds) else {
                    continue;
                };
                let Some(next) = Instant::now().checked_add(wait) else {
                    continue;
                };
                self.refresh_due.insert(id.clone(), next);
                self.press(ctx, id, Trigger::Refresh);
            }
        }
    }

    fn hardware_events(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        if let Some(handle) = &self.hardware {
            while let Ok(event) = handle.events.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                hardware::Event::Connected { name, rows, cols } => {
                    self.deck_status = Some(format!("{name} ({rows} × {cols})"));
                    self.deck_shape = Some((rows, cols));
                    self.log(true, format!("deck connected: {name}"));
                }
                hardware::Event::Disconnected => {
                    self.deck_status = None;
                    self.deck_shape = None;
                    self.log(false, "deck disconnected");
                }
                hardware::Event::Pressed { row, col } => {
                    self.saver.last_press = Instant::now();
                    // This press only wakes the deck: nobody can see what they're pressing yet.
                    if self.asleep {
                        self.asleep = false;
                        continue;
                    }
                    if self.saver.playing.is_some() {
                        self.stop_playing();
                        continue;
                    }
                    if self.in_folder() && (row, col) == BACK_SLOT {
                        self.leave_folder();
                        continue;
                    }
                    let id = self.key_id(row, col);
                    self.press(ctx, id, Trigger::Press);
                }
            }
        }
    }

    /// Keeps the physical deck showing the current page.
    fn hardware_sync(&mut self) {
        let Some(handle) = &self.hardware else {
            return;
        };
        let brightness = self.ws.settings.hardware.brightness;
        if brightness != self.last_brightness {
            self.last_brightness = brightness;
            let _ = handle.commands.send(hardware::Command::Brightness(brightness));
        }
        if self.asleep != self.last_asleep {
            self.last_asleep = self.asleep;
            if let Some(handle) = &self.hardware {
                let _ = handle.commands.send(hardware::Command::Asleep(self.asleep));
            }
        }
        let playing = self.saver.playing.is_some();
        if playing != self.last_saver_playing {
            self.last_saver_playing = playing;
            if !playing && let Some(handle) = &self.hardware {
                let _ = handle.commands.send(hardware::Command::SaverStop);
            }
        }
        // Nothing to draw on until a deck is plugged in, so the keys aren't built for nothing.
        if self.deck_shape.is_none() {
            return;
        }
        let visuals = self.current_visuals();
        if visuals != self.last_visuals {
            if let Some(handle) = &self.hardware {
                let _ = handle.commands.send(hardware::Command::Show(visuals.clone()));
            }
            self.last_visuals = visuals;
        }
    }

    fn current_visuals(&self) -> Vec<hardware::KeyVisual> {
        let Some(page) = self.current_page() else {
            return Vec::new();
        };
        let back = self.in_folder().then(|| hardware::KeyVisual {
            row: BACK_SLOT.0,
            col: BACK_SLOT.1,
            title: "Back".into(),
            icon: "⬅".into(),
            background: String::new(),
            color: "#3A4252".into(),
            text_color: String::new(),
        });
        page.keys
            .iter()
            .filter(|key| !self.in_folder() || (key.row, key.col) != BACK_SLOT)
            .map(|key| {
                let (title, color) = self.key_look(key);
                hardware::KeyVisual {
                    row: key.row,
                    col: key.col,
                    title,
                    icon: key.icon.clone(),
                    background: self.background_path(&key.background),
                    color,
                    text_color: key.text_color.clone(),
                }
            })
            .chain(back)
            .collect()
    }

    /// The full path of a stored picture, or nothing if the name isn't one.
    pub fn background_path(&self, name: &str) -> String {
        self.ws.paths.image(name).map_or_else(String::new, |path| path.to_string_lossy().into_owned())
    }

    /// Hides the window. The deck, timers and auto-lock all carry on.
    fn hide_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.hidden = true;
        #[cfg(target_os = "macos")]
        {
            crate::macos::set_in_dock(false);
            self.was_active = crate::macos::is_active();
        }
    }

    pub fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        #[cfg(target_os = "macos")]
        crate::macos::set_in_dock(true);
        ctx.request_repaint();
    }

    /// Saves everything, wipes the vault key and clears the deck, then exits straight away. It
    /// works while the window is hidden, when the normal close path doesn't run.
    pub fn quit_now(&mut self) -> ! {
        self.shut_down();
        std::process::exit(0);
    }

    fn shut_down(&mut self) {
        let dirty: Vec<String> = self.dirty_profiles.keys().cloned().collect();
        // Only ever an autosave: it comes back as unsaved work at the next start.
        for id in dirty {
            let _ = self.ws.autosave_profile(&id);
        }
        if self.dirty_settings.is_some() {
            let _ = self.ws.save_settings();
        }
        if let Ok(mut vault) = self.engine.vault.lock() {
            vault.lock();
        }
        audit(&self.ws.paths, "app", "closed");
        // Dropping the handle tells the deck's thread to clear the keys. Give it a moment.
        if self.hardware.take().is_some() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Requests from the menu bar item, or from the system opening the app again.
    fn outside_requests(&mut self, ctx: &egui::Context) {
        match self.tray.as_ref().and_then(tray::Tray::poll) {
            Some(tray::Request::Show) => self.show_window(ctx),
            Some(tray::Request::Quit) => self.quit_now(),
            None => {}
        }
        #[cfg(target_os = "macos")]
        if self.hidden && self.has_lock {
            let active = crate::macos::is_active();
            if active && !self.was_active {
                self.show_window(ctx);
            }
            self.was_active = active;
        }
    }

    /// What a key shows right now, taking toggles and live values into account.
    pub fn key_look(&self, key: &Key) -> (String, String) {
        let id = self.key_id(key.row, key.col);
        if self.saver.armed.as_ref().is_some_and(|armed| armed.id == id) {
            return (format!("{}: On", key.title), key.color.clone());
        }
        if let Some(live) = self.live_titles.get(&id) {
            return (live.clone(), key.color.clone());
        }
        if self.flipped.contains(&id) {
            let title = if key.alt_title.is_empty() { &key.title } else { &key.alt_title };
            let color = if key.alt_color.is_empty() { &key.color } else { &key.alt_color };
            return (title.clone(), color.clone());
        }
        (key.title.clone(), key.color.clone())
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        egui::Panel::top("top_bar")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(14, 8)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let ctx = ui.ctx().clone();
                    let logo = self.logo.get_or_insert_with(|| {
                        let picture = crate::icon::render(64);
                        let color = egui::ColorImage::from_rgba_unmultiplied([64, 64], picture.as_raw());
                        ctx.load_texture("logo", color, egui::TextureOptions::LINEAR)
                    });
                    ui.add(egui::Image::from_texture((logo.id(), egui::vec2(28.0, 28.0))));
                    ui.label(RichText::new("Pyro Porch").strong().size(16.0));
                    ui.add_space(18.0);
                    for (view, label) in [
                        (View::Deck, "Deck"),
                        (View::Plugins, "Plugins"),
                        (View::Vault, "Vault"),
                        (View::Assistant, "AI Assistant"),
                        (View::Settings, "Settings"),
                    ] {
                        let selected = self.view == view;
                        let text = if selected { RichText::new(label).color(Color32::WHITE) } else { RichText::new(label) };
                        let fill = if selected { p.accent } else { Color32::TRANSPARENT };
                        if ui.add(egui::Button::new(text).fill(fill).stroke(egui::Stroke::NONE)).clicked() {
                            if self.view == View::Vault && view != View::Vault {
                                self.vault_form.passphrase.wipe();
                            }
                            self.view = view;
                            if view == View::Plugins {
                                self.engine.refresh_plugins();
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.vault_chip(ui, p);
                        let changes = self.active_has_changes();
                        let label = if changes { "Save" } else { "Saved" };
                        let text = if changes { RichText::new(label).color(Color32::WHITE) } else { RichText::new(label) };
                        let fill = if changes { p.accent } else { Color32::TRANSPARENT };
                        let button = ui
                            .add_enabled(changes, egui::Button::new(text).fill(fill))
                            .on_hover_text("Puts this profile's changes in its file. Until then they're kept in an autosave file, and assistants don't see them.");
                        if button.clicked() {
                            self.save_active_profile();
                        }
                        if self.asleep && ui.button("Wake the deck").clicked() {
                            self.asleep = false;
                        }
                        if self.saver.playing.is_some() {
                            if ui.button("Stop the screen saver").clicked() {
                                self.stop_playing();
                            }
                        } else if self.saver.armed.is_some() && ui.button("Turn the screen saver off").clicked() {
                            self.saver_off();
                        }
                        let waiting = self.pending.hosts.len() + self.pending.secrets.len();
                        if waiting > 0 {
                            let text = RichText::new(format!("🔔 {waiting} waiting for you")).color(Color32::BLACK);
                            if ui.add(egui::Button::new(text).fill(p.warn)).clicked() {
                                self.show_requests = true;
                            }
                        }
                        let status = self.deck_status.clone().unwrap_or_else(|| "On-screen deck".into());
                        ui.label(RichText::new(status).color(p.muted));
                    });
                });
            });
    }

    fn vault_chip(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        let state = self.engine.vault.lock().map_or(VaultState::Locked, |v| v.state());
        let (text, color) = match state {
            VaultState::NotCreated => ("🔒 Vault not set up", p.muted),
            VaultState::Locked => ("🔒 Vault locked", p.warn),
            VaultState::Unlocked => ("🔓 Vault open", p.ok),
        };
        if ui.add(egui::Button::new(RichText::new(text).color(color)).fill(Color32::TRANSPARENT)).clicked() {
            self.view = View::Vault;
        }
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else {
            return;
        };
        shot.frames += 1;
        ctx.request_repaint();
        if shot.frames >= SCREENSHOT_FRAMES && !shot.requested {
            shot.requested = true;
            let (path, done) = (shot.path.clone(), shot.done.clone());
            let wake = ctx.clone();
            ctx.request_screenshot(move |image| {
                let result = save_png(&path, &image);
                if let Ok(mut slot) = done.lock() {
                    *slot = Some(result);
                }
                wake.request_repaint();
            });
        }
        let finished = shot.done.lock().ok().and_then(|mut slot| slot.take());
        match finished {
            Some(Ok(())) => {
                let id = self.active_id();
                let _ = self.ws.save_profile(&id);
                std::process::exit(0);
            }
            Some(Err(error)) => {
                eprintln!("pyroporch: couldn't save the screenshot: {error}");
                std::process::exit(1);
            }
            None => {}
        }
    }
}

fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let [width, height] = image.size;
    let mut bytes = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        // The window is opaque; drop any alpha the driver reports so the picture isn't see-through.
        bytes.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), 255]);
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    image::save_buffer(path, &bytes, width as u32, height as u32, image::ColorType::Rgba8).map_err(|e| e.to_string())
}

impl App {
    /// Work that has to carry on even while the window is hidden.
    pub fn background(&mut self, ctx: &egui::Context) {
        if self.shot.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
        self.outside_requests(ctx);
        self.finish_runs(ctx);
        self.hardware_events(ctx);
        self.tick(ctx);
        self.saver_tick(ctx);
        if self.ws.settings.ui.tray_icon != self.last_tray_style {
            self.last_tray_style = self.ws.settings.ui.tray_icon.clone();
            if let Some(tray) = &self.tray
                && let Err(error) = tray.set_style(&self.last_tray_style)
            {
                self.log(false, format!("couldn't change the menu bar icon: {error}"));
            }
        }
        self.autosave();
        self.hardware_sync();
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Draws the whole window.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.has_lock && self.ws.settings.run_in_background && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_window(&ctx);
        }
        self.top_bar(ui);
        match self.view {
            View::Deck => self.deck_view(ui),
            View::Plugins => self.plugins_view(ui),
            View::Vault => self.vault_view(ui),
            View::Assistant => self.assistant_view(ui),
            View::Settings => self.settings_view(ui),
        }
        self.requests_window(&ctx);
        self.secret_window(&ctx);
        self.crop_window(&ctx);
        self.import_window(&ctx);
        self.screenshot_step(&ctx);
    }

    /// Opens a given screen at start, for the `--view` option.
    pub fn start_on(&mut self, view: &str) {
        match view {
            "plugins" => self.view = View::Plugins,
            "vault" => self.view = View::Vault,
            "assistant" => self.view = View::Assistant,
            "settings" => self.view = View::Settings,
            "requests" => self.show_requests = true,
            "new-secret" => self.secret_dialog = Some(SecretDialog::default()),
            "fit-picture" => self.start_demo_crop(),
            "hotkey" => self.selected = Some((1, 0)),
            "folder" => self.open_folder((1, 4)),
            "second-page" => self.page = 1,
            "use-mode" => self.use_mode = true,
            "collapsed" => self.ws.settings.ui.collapsed_groups = vec!["core.navigation".into(), "core.deck".into()],
            "several-actions" => self.selected = Some((1, 2)),
            "asleep" => self.asleep = true,
            name if name.starts_with("saver:") => {
                let animation = name.trim_start_matches("saver:");
                let settings = serde_json::json!({"animation": animation}).as_object().cloned().unwrap_or_default();
                let ctx = self.ctx_for_crop.clone();
                self.preview_saver(&ctx, "pyroporch.screensaver".into(), settings);
            }
            _ => {}
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) && self.active_has_changes() {
            self.save_active_profile();
        }
        self.background(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shut_down();
    }
}
