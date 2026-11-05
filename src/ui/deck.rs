//! The main screen: the key grid, the action library and the key's settings.

use super::theme::{Palette, color_hex, parse_color};
use super::crop::{CropDialog, CropResult, Opened, encode_png, open_picture};
use super::{App, KeyId, KeyLook, SecretDialog, SecretTarget, hotkey};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use pyroporch_core::config::{BACK_SLOT, Key, MAX_FOLDER_DEPTH, Page, Preset, Profile, Step};
use pyroporch_core::engine::{Block, SLEEP_ICON};
use pyroporch_core::label::{FittedLabel, KeyLayout, LINE_HEIGHT, TitlePosition, fit_label};
use pyroporch_core::policy::Grants;
use pyroporch_core::security::{find_refs, plaintext_findings, reference_name};
use pyroporch_core::transfer::store_image;
use pyroporch_sdk::{ActionSpec, Field, FieldKind, Risk, Rule, RuleTest, Trigger};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

const KEY_GAP: f32 = 12.0;
const MAX_FITTED_LABELS: usize = 1000;
/// How big a drawn icon is in the icon picker.
const ICON_BUTTON_SIZE: f32 = 22.0;
/// The most favourite colours kept. Adding another lets go of the oldest.
const MAX_FAVORITE_COLORS: usize = 32;
const MAX_KEY_SIZE: f32 = 112.0;
const FLASH: Duration = Duration::from_millis(600);
const GRID_CHOICES: [(u32, u32); 6] = [(2, 3), (3, 3), (3, 5), (4, 5), (4, 8), (5, 8)];
/// The ones a status is most likely to want, for a key that changes its look by itself.
pub const STATUS_ICON_CHOICES: [&str; 6] = ["✅", "⚠", "❌", "🔥", "⛔", "❓"];
const ICON_CHOICES: [&str; 26] = [
    "🔗", "🌐", "▶", "⏸", "⏮", "⏭", "🔊", "🔉", "🔇", "🎤", "📷", "⌨", "✏", "⚙", "🚀", "⭐", "💡", "🏠", "📁", "📄", "📅", "✉", "🔒", "➡", "⬅", "⏱",
];

/// What is being dragged.
#[derive(Clone)]
pub(super) enum Drag {
    Action(String),
    Preset(Preset),
    Key(u32, u32),
    Folder,
}

/// What a drop would do here, in the words shown on the key while dragging. Nothing if it can't.
pub(super) fn drop_hint(payload: &Drag, slot: (u32, u32), has_key: bool, is_folder: bool) -> Option<&'static str> {
    match payload {
        Drag::Action(_) | Drag::Preset(_) if is_folder => Some("Put in folder"),
        Drag::Action(_) | Drag::Preset(_) if has_key => Some("+ Add to this key"),
        Drag::Action(_) | Drag::Preset(_) => Some("New key"),
        Drag::Key(row, col) if (*row, *col) == slot => None,
        Drag::Key(..) if is_folder => Some("Move into folder"),
        Drag::Key(..) if has_key => Some("Swap keys"),
        Drag::Key(..) => Some("Move here"),
        Drag::Folder if has_key => None,
        Drag::Folder => Some("New folder"),
    }
}

const BACK_COLOR: &str = "#3A4252";
const FOLDER_ICON: &str = "📁";
const MAX_STEP_WAIT_MS: u64 = 600_000;
const MODE_FRAME_GAP: f32 = 7.0;
const GROUP_HEADING_HEIGHT: f32 = 22.0;
const DRAWN_ICON_PREFIX: &str = "builtin:";

fn empty_key(row: u32, col: u32) -> Key {
    Key {
        row,
        col,
        ..Key::default()
    }
}

fn default_settings(spec: &ActionSpec) -> Map<String, Value> {
    spec.fields
        .iter()
        .filter(|f| !f.default.is_null())
        .map(|f| (f.key.clone(), f.default.clone()))
        .collect()
}

/// Black or white, whichever reads better on this background.
fn readable_on(color: Color32) -> Color32 {
    let light = 0.299 * f32::from(color.r()) + 0.587 * f32::from(color.g()) + 0.114 * f32::from(color.b());
    if light > 150.0 { Color32::from_rgb(0x14, 0x17, 0x1C) } else { Color32::WHITE }
}

impl App {
    /// Throws away the flashes that have finished.
    pub fn clear_finished_flashes(&mut self) {
        self.flashes.retain(|_, (at, _)| at.elapsed() < FLASH);
    }

    pub fn deck_view(&mut self, ui: &mut Ui) {
        let p = self.palette();
        egui::Panel::right("library")
            .exact_size(270.0)
            .frame(egui::Frame::new().fill(p.panel).inner_margin(12).stroke(Stroke::new(1.0, p.border)))
            .show(ui, |ui| self.library(ui, p));
        self.dropped_picture(ui);
        egui::Panel::bottom("inspector")
            .exact_size(296.0)
            .frame(egui::Frame::new().fill(p.panel).inner_margin(12).stroke(Stroke::new(1.0, p.border)))
            .show(ui, |ui| self.inspector(ui, p));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(14))
            .show(ui, |ui| {
                self.deck_toolbar(ui, p);
                ui.add_space(6.0);
                self.mode_banner(ui, p);
                ui.add_space(6.0);
                self.page_tabs(ui, p);
                ui.add_space(8.0);
                self.config_banner(ui, p);
                self.grid(ui, p);
            });
    }

    /// The colour that means this mode. Edit follows the app's accent; Use is always green.
    fn mode_color(&self, p: Palette) -> Color32 {
        if self.use_mode { p.ok } else { p.accent }
    }

    /// Says which mode the deck is in and what clicking does there, with the switch beside it.
    fn mode_banner(&mut self, ui: &mut Ui, p: Palette) {
        let color = self.mode_color(p);
        let (name, explain) = if self.use_mode {
            ("USE MODE", "Clicking a key runs it. To change or remove keys, switch to Edit.")
        } else {
            ("EDIT MODE", "Click a key to set it up, drag to move it, right-click to remove it. Double-click to try it.")
        };
        let mut switch_to = None;
        egui::Frame::new()
            .fill(color.gamma_multiply(0.2))
            .stroke(Stroke::new(1.5, color))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).strong().color(color));
                    ui.label(RichText::new(explain).small());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let option = |ui: &mut Ui, text: &str, on: bool, fill: Color32| {
                            let label = if on { RichText::new(text).strong().color(Color32::WHITE) } else { RichText::new(text) };
                            ui.add(egui::Button::new(label).fill(if on { fill } else { p.raised })).clicked()
                        };
                        if option(ui, "Use keys", self.use_mode, p.ok) {
                            switch_to = Some(true);
                        }
                        if option(ui, "Edit keys", !self.use_mode, p.accent) {
                            switch_to = Some(false);
                        }
                    });
                });
            });
        if let Some(use_mode) = switch_to {
            self.use_mode = use_mode;
        }
    }

    /// Replaces the action list in Use mode, because nothing can be added from there.
    fn library_in_use_mode(&mut self, ui: &mut Ui, p: Palette) {
        ui.label(RichText::new("Actions").strong().size(15.0));
        ui.label(RichText::new("You're in Use mode, so keys run when clicked. Switch to Edit to add actions to them.").color(p.muted));
    }

    /// Replaces a key's settings in Use mode, so it's plain why they aren't there.
    fn inspector_in_use_mode(&mut self, ui: &mut Ui, p: Palette) {
        let mut switch = false;
        ui.columns(2, |columns| {
            columns[0].vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.label(RichText::new("You're in Use mode").strong().color(p.ok));
                ui.label(RichText::new("Clicking a key runs it. Changing or removing keys happens in Edit mode.").color(p.muted));
                ui.add_space(4.0);
                if ui.button("Switch to Edit").clicked() {
                    switch = true;
                }
            });
            self.activity(&mut columns[1], p);
        });
        if switch {
            self.use_mode = false;
        }
    }

    fn config_banner(&mut self, ui: &mut Ui, p: Palette) {
        for error in self.ws.errors.clone() {
            ui.colored_label(p.bad, format!("⚠ {error}"));
        }
    }

    fn deck_toolbar(&mut self, ui: &mut Ui, p: Palette) {
        ui.horizontal(|ui| {
            let current = self.profile().map_or_else(String::new, |x| x.name.clone());
            let mut chosen = None;
            let label = ui.label(RichText::new("Profile").color(p.muted));
            egui::ComboBox::from_id_salt("profile")
                .selected_text(current)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (id, profile) in &self.ws.profiles {
                        if ui.selectable_label(*id == self.ws.settings.active_profile, &profile.name).clicked() {
                            chosen = Some(id.clone());
                        }
                    }
                })
                .response
                .labelled_by(label.id);
            if let Some(id) = chosen {
                self.switch_profile(id);
            }
            if !self.use_mode && ui.button("+ Profile").on_hover_text("Add a profile").clicked() {
                self.new_profile_name = Some(String::new());
            }
            if let Some(mut name) = self.new_profile_name.take() {
                let response = ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0).hint_text("Name"));
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && !name.trim().is_empty() {
                    let profile = Profile {
                        name: name.trim().to_string(),
                        ..Profile::default()
                    };
                    match self.ws.add_unsaved_profile(profile) {
                        Ok(id) => self.switch_profile(id),
                        Err(error) => self.log(false, error),
                    }
                } else if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && !response.lost_focus() {
                    // Only until it has focus: asking again would hide that focus was given up,
                    // which is how pressing return shows.
                    if !response.has_focus() {
                        response.request_focus();
                    }
                    self.new_profile_name = Some(name);
                }
            }

            let (rows, cols) = self.profile().map_or((3, 5), |x| (x.rows, x.cols));
            let mut new_size = None;
            if self.use_mode {
                return;
            }
            let label = ui.label(RichText::new("Grid").color(p.muted));
            egui::ComboBox::from_id_salt("grid_size")
                .selected_text(format!("{rows} × {cols} keys"))
                .show_ui(ui, |ui| {
                    for (r, c) in GRID_CHOICES {
                        if ui.selectable_label((r, c) == (rows, cols), format!("{r} × {c} keys")).clicked() {
                            new_size = Some((r, c));
                        }
                    }
                })
                .response
                .labelled_by(label.id);
            if let Some((r, c)) = new_size
                && let Some(profile) = self.profile_mut()
            {
                profile.rows = r;
                profile.cols = c;
                self.mark_profile_dirty();
            }
            if ui.add_enabled(self.can_undo(), egui::Button::new("↩ Undo")).on_hover_text("Take back the last change to this profile").clicked() {
                self.undo();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((r, c)) = self.deck_shape
                    && (r, c) != (rows, cols)
                    && ui.button(format!("Match my deck ({r} × {c})")).clicked()
                    && let Some(profile) = self.profile_mut()
                {
                    profile.rows = r;
                    profile.cols = c;
                    self.mark_profile_dirty();
                }
            });
        });
    }

    /// Shown instead of the page tabs inside a folder, because a folder is its own screen.
    fn folder_trail(&mut self, ui: &mut Ui, p: Palette) {
        let mut names = Vec::new();
        let mut screen = self.profile().and_then(|x| x.pages.get(self.page));
        if let Some(page) = screen {
            names.push(page.name.clone());
        }
        for (row, col) in self.folder.clone() {
            let key = screen.and_then(|page| page.key_at(row, col));
            names.push(key.map_or_else(String::new, |key| key.title.clone()));
            screen = key.and_then(|key| key.folder.as_deref());
        }
        ui.horizontal(|ui| {
            if ui.button("⬅ Out of folder").clicked() {
                self.leave_folder();
            }
            ui.label(RichText::new(names.join("  ›  ")).color(p.muted));
        });
    }

    fn page_tabs(&mut self, ui: &mut Ui, p: Palette) {
        if self.in_folder() {
            self.folder_trail(ui, p);
            return;
        }
        let names: Vec<String> = self.profile().map_or_else(Vec::new, |x| x.pages.iter().map(|page| page.name.clone()).collect());
        ui.horizontal(|ui| {
            for (index, name) in names.iter().enumerate() {
                let on = index == self.page;
                let text = if on { RichText::new(name).strong() } else { RichText::new(name).color(p.muted) };
                let button = egui::Button::new(text).fill(if on { p.raised } else { Color32::TRANSPARENT });
                if ui.add(button).clicked() {
                    self.page = index;
                    self.selected = None;
                    self.draft = None;
                }
            }
            if self.use_mode {
                return;
            }
            if ui.small_button("+").on_hover_text("Add a page").clicked() {
                self.new_page_name = Some(String::new());
            }
            if let Some(mut name) = self.new_page_name.take() {
                let response = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0).hint_text("Page name"));
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && !name.trim().is_empty() {
                    if let Some(profile) = self.profile_mut() {
                        profile.pages.push(Page {
                            name: name.trim().to_string(),
                            keys: Vec::new(),
                        });
                        self.page = profile.pages.len() - 1;
                    }
                    self.mark_profile_dirty();
                } else if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && !response.lost_focus() {
                    // Only until it has focus: asking again would hide that focus was given up,
                    // which is how pressing return shows.
                    if !response.has_focus() {
                        response.request_focus();
                    }
                    self.new_page_name = Some(name);
                }
            }
            if names.len() > 1 && ui.small_button("Delete page").clicked() {
                self.confirm_delete_page = Some(self.page);
            }
        });
    }

    /// Asks before a page goes, as it can hold a lot of work, with folders full of keys in it.
    pub fn delete_page_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.confirm_delete_page else {
            return;
        };
        let Some(page) = self.profile().and_then(|profile| profile.pages.get(index)) else {
            self.confirm_delete_page = None;
            return;
        };
        let (name, keys) = (page.name.clone(), page.keys.len());
        let p = self.palette();
        let mut delete = false;
        let mut keep = false;
        let modal = egui::Modal::new(egui::Id::unique("delete_page_dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(format!("⚠ Delete the page \"{name}\"?")).heading().strong().color(p.bad));
            ui.add_space(6.0);
            let holds = match keys {
                0 => "It has no keys on it.".to_string(),
                1 => "The 1 key on it goes with it, along with anything inside.".to_string(),
                keys => format!("All {keys} keys on it go with it, along with anything inside folders."),
            };
            ui.label(holds);
            ui.label(RichText::new("Undo can bring it back until the app closes. After that, it's gone for good.").small().color(p.muted));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Yes, delete this page").strong().color(Color32::WHITE)).fill(p.bad)).clicked() {
                    delete = true;
                }
                if ui.button("Keep page").clicked() {
                    keep = true;
                }
            });
        });
        if delete {
            self.delete_page(index);
        }
        if delete || keep || modal.should_close() {
            self.confirm_delete_page = None;
        }
    }

    fn delete_page(&mut self, index: usize) {
        let Some(profile) = self.profile_mut() else {
            return;
        };
        if profile.pages.len() < 2 {
            return;
        }
        profile.pages.remove(index);
        if self.page >= index {
            self.page = self.page.saturating_sub(1);
        }
        self.leave_all_folders();
        self.mark_profile_dirty();
    }

    /// Every key dark, or playing the screen saver. Clicking any of them wakes the deck, and does
    /// nothing else.
    fn sleeping_grid(&mut self, ui: &mut Ui, rect_for: impl Fn(u32, u32) -> egui::Rect, rows: u32, cols: u32) {
        let mut wake = false;
        let saver_playing = self.saver.playing.is_some() && !self.asleep;
        for row in 0..rows {
            for col in 0..cols {
                let rect = rect_for(row, col);
                let id = ui.make_persistent_id(("asleep", row, col));
                let response = ui.interact(rect, id, Sense::click());
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Sleeping key {row} {col}")));
                if saver_playing {
                    self.paint_saver_key(ui, rect, row, col);
                } else {
                    let radius = CornerRadius::same((rect.width() * 0.14) as u8);
                    ui.painter().rect_filled(rect, radius, Color32::from_gray(6));
                }
                wake |= response.clicked();
            }
        }
        if wake {
            self.asleep = false;
            self.stop_playing();
        }
    }

    fn grid(&mut self, ui: &mut Ui, p: Palette) {
        let Some(profile) = self.profile() else {
            return;
        };
        let (rows, cols) = (profile.rows, profile.cols);
        // A screen saver is drawn for the deck, which may be a different shape from this profile.
        let (rows, cols) = if self.asleep { (rows, cols) } else { self.saver_shape().unwrap_or((rows, cols)) };
        let space = ui.available_rect_before_wrap();
        let area = space.size();
        let side = ((area.x - KEY_GAP * (cols as f32 - 1.0)) / cols as f32)
            .min((area.y - KEY_GAP * (rows as f32 - 1.0)) / rows as f32)
            .clamp(36.0, MAX_KEY_SIZE);
        let total = Vec2::new(
            side * cols as f32 + KEY_GAP * (cols as f32 - 1.0),
            side * rows as f32 + KEY_GAP * (rows as f32 - 1.0),
        );
        let origin = space.min + Vec2::new(((area.x - total.x) / 2.0).max(0.0), ((area.y - total.y) / 2.0).max(0.0));
        ui.allocate_rect(space, Sense::hover());
        // A frame around the deck in the mode's colour: solid to use, dashed while it's being edited.
        let frame = egui::Rect::from_min_size(origin, total).expand(MODE_FRAME_GAP);
        let color = self.mode_color(p);
        if self.use_mode {
            ui.painter().rect_stroke(frame, 12, Stroke::new(2.0, color), StrokeKind::Outside);
        } else {
            let corners = [frame.left_top(), frame.right_top(), frame.right_bottom(), frame.left_bottom(), frame.left_top()];
            ui.painter().extend(egui::Shape::dashed_line(&corners, Stroke::new(2.0, color), 9.0, 6.0));
        }
        if self.asleep || self.saver.playing.is_some() {
            let rect_for = |row: u32, col: u32| {
                egui::Rect::from_min_size(origin + Vec2::new(col as f32 * (side + KEY_GAP), row as f32 * (side + KEY_GAP)), Vec2::splat(side))
            };
            self.sleeping_grid(ui, rect_for, rows, cols);
            return;
        }

        let mut dropped: Option<(u32, u32, Arc<Drag>)> = None;
        let mut clicked = None;
        let mut pressed = None;
        let mut clear = None;
        let mut go_back = false;
        let mut edit_key = None;
        for row in 0..rows {
            for col in 0..cols {
                let min = origin + Vec2::new(col as f32 * (side + KEY_GAP), row as f32 * (side + KEY_GAP));
                let rect = egui::Rect::from_min_size(min, Vec2::splat(side));
                let id = ui.make_persistent_id(("key", row, col));
                let response = ui.interact(rect, id, Sense::click_and_drag());
                let back_key = self.in_folder() && (row, col) == BACK_SLOT;
                if back_key {
                    self.draw_back_key(ui, rect, &response, p);
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Back"));
                    if response.clicked() {
                        go_back = true;
                    }
                    continue;
                }
                // The key is looked up once and drawn from where it is, not copied out.
                let key = self.current_page().and_then(|page| page.key_at(row, col));
                self.draw_key(ui, rect, row, col, &response, p, key);
                let name = key.map_or_else(|| format!("Empty key {row} {col}"), |key| self.key_look(key).title);
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
                if response.clicked() {
                    clicked = Some((row, col));
                }
                if response.double_clicked() && !self.use_mode {
                    pressed = Some((row, col));
                }
                let has_key = key.is_some();
                if has_key && !self.use_mode {
                    response.dnd_set_drag_payload(Drag::Key(row, col));
                    response.clone().on_hover_cursor(egui::CursorIcon::Grab);
                    response.context_menu(|ui| {
                        if ui.button("Remove key").clicked() {
                            clear = Some((row, col));
                            ui.close();
                        }
                    });
                } else if has_key {
                    response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                    // Someone reaching for this in Use mode wants Edit mode.
                    response.context_menu(|ui| {
                        if ui.button("Edit this key").clicked() {
                            edit_key = Some((row, col));
                            ui.close();
                        }
                    });
                }
                if let Some(payload) = response.dnd_release_payload::<Drag>() {
                    dropped = Some((row, col, payload));
                }
            }
        }
        if go_back {
            self.leave_folder();
            return;
        }
        if let Some(slot) = edit_key {
            self.use_mode = false;
            self.selected = Some(slot);
            self.draft = None;
        }
        if let Some((row, col)) = clicked {
            if self.use_mode {
                if self.current_page().is_some_and(|page| page.key_at(row, col).is_none()) {
                    self.log(true, "that key is empty: switch to Edit to set it up");
                }
                pressed = Some((row, col));
            } else if self.selected != Some((row, col)) {
                self.selected = Some((row, col));
                self.draft = None;
            }
        }
        if let Some((row, col)) = pressed {
            let id = self.key_id(row, col);
            self.press(ui.ctx(), id, Trigger::Press);
        }
        if let Some((row, col)) = clear {
            self.remove_key(row, col);
        }
        if let Some((row, col, payload)) = dropped {
            self.handle_drop(row, col, &payload);
        }
    }

    /// The screen showing now: the page itself, or the folder that's open on it.
    pub fn current_page(&self) -> Option<&Page> {
        self.profile().and_then(|x| x.pages.get(self.page)).and_then(|page| page.descend(&self.folder))
    }

    pub fn current_page_mut(&mut self) -> Option<&mut Page> {
        let (page, folder) = (self.page, self.folder.clone());
        self.profile_mut().and_then(|x| x.pages.get_mut(page)).and_then(|page| page.descend_mut(&folder))
    }

    fn remove_key(&mut self, row: u32, col: u32) {
        if let Some(page) = self.current_page_mut() {
            page.remove_at(row, col);
        }
        if self.selected == Some((row, col)) {
            self.draft = None;
        }
        self.mark_profile_dirty();
    }

    /// Moves a key into a folder on the same screen.
    fn move_into_folder(&mut self, from: (u32, u32), folder_at: (u32, u32)) {
        let (rows, cols) = self.profile().map_or((3, 5), |x| (x.rows, x.cols));
        // A folder being moved in brings its own contents, so the whole tree has to stay
        // within the depth the app can load. Otherwise the profile could never be saved again.
        let brought = self
            .current_page()
            .and_then(|page| page.key_at(from.0, from.1))
            .map_or(0, |key| key.folder.as_ref().map_or(0, |folder| folder.depth() + 1));
        // Going into the folder is one level further down than this screen.
        if brought > 0 && self.folder.len() + 1 + brought > MAX_FOLDER_DEPTH {
            self.log(false, format!("folders can only go {MAX_FOLDER_DEPTH} deep"));
            return;
        }
        let Some(page) = self.current_page_mut() else {
            return;
        };
        let free = page.key_at(folder_at.0, folder_at.1).and_then(|key| key.folder.as_ref()).and_then(|folder| folder.free_folder_slot(rows, cols));
        let Some(slot) = free else {
            self.log(false, "that folder is full");
            return;
        };
        if let Some(mut moving) = page.remove_at(from.0, from.1) {
            moving.row = slot.0;
            moving.col = slot.1;
            if let Some(folder) = page.key_at_mut(folder_at.0, folder_at.1).and_then(|key| key.folder.as_mut()) {
                folder.keys.push(moving);
            }
        }
        self.selected = Some(folder_at);
        self.draft = None;
    }

    fn is_folder_key(&self, row: u32, col: u32) -> bool {
        self.current_page().and_then(|page| page.key_at(row, col)).is_some_and(|key| key.folder.is_some())
    }

    pub(super) fn handle_drop(&mut self, row: u32, col: u32, payload: &Drag) {
        // A folder's first slot is its back key.
        if self.in_folder() && (row, col) == BACK_SLOT {
            return;
        }
        // Anything dropped on a folder goes inside it.
        let onto_folder = self.is_folder_key(row, col);
        match payload {
            Drag::Folder => {
                if self.folder.len() >= MAX_FOLDER_DEPTH {
                    self.log(false, format!("folders can only go {MAX_FOLDER_DEPTH} deep"));
                    return;
                }
                let empty = self.current_page().is_some_and(|page| page.key_at(row, col).is_none());
                if !empty {
                    self.log(false, "drop a folder on an empty key");
                    return;
                }
                let key = Key {
                    title: "Folder".into(),
                    icon: FOLDER_ICON.into(),
                    color: BACK_COLOR.into(),
                    folder: Some(Box::new(Page {
                        name: "Folder".into(),
                        keys: Vec::new(),
                    })),
                    ..empty_key(row, col)
                };
                if let Some(page) = self.current_page_mut() {
                    page.keys.push(key);
                }
                self.selected = Some((row, col));
                self.draft = None;
            }
            Drag::Key(from_row, from_col) => {
                if (*from_row, *from_col) == (row, col) {
                    return;
                }
                if onto_folder {
                    self.move_into_folder((*from_row, *from_col), (row, col));
                    self.mark_profile_dirty();
                    return;
                }
                if let Some(page) = self.current_page_mut() {
                    // Dropping on another key swaps the two.
                    let other = page.remove_at(row, col);
                    if let Some(mut moving) = page.remove_at(*from_row, *from_col) {
                        moving.row = row;
                        moving.col = col;
                        page.keys.push(moving);
                    }
                    if let Some(mut other) = other {
                        other.row = *from_row;
                        other.col = *from_col;
                        page.keys.push(other);
                    }
                }
                self.selected = Some((row, col));
                self.draft = None;
            }
            Drag::Action(action) => {
                let Some(spec) = self.engine.spec(action) else {
                    return;
                };
                let step = Step {
                    action: action.clone(),
                    settings: default_settings(&spec),
                    delay_ms: 0,
                };
                if onto_folder {
                    self.add_inside_folder((row, col), step, &spec.name, &spec.icon, "", true);
                } else {
                    self.add_step(row, col, step, &spec.name, &spec.icon, "", true);
                }
            }
            Drag::Preset(preset) => {
                let step = Step {
                    action: preset.action.clone(),
                    settings: preset.settings.clone(),
                    delay_ms: 0,
                };
                // A preset's settings come from a file, perhaps written by an assistant, so the
                // person hasn't seen them yet. One holding a password in plain text is not
                // added at all: it must never reach the config file.
                if self.engine.check_step(&step).iter().any(|block| matches!(block, Block::PlainCredential(_)))
                    || !plaintext_findings(None, &step.settings).is_empty()
                {
                    self.log(false, "that preset holds a password in plain text, so it was not added");
                    return;
                }
                if onto_folder {
                    self.add_inside_folder((row, col), step, &preset.name, &preset.icon, &preset.color, false);
                } else {
                    self.add_step(row, col, step, &preset.name, &preset.icon, &preset.color, false);
                }
            }
        }
        self.mark_profile_dirty();
    }

    #[allow(clippy::too_many_arguments)]
    fn add_inside_folder(&mut self, folder_at: (u32, u32), step: Step, title: &str, icon: &str, color: &str, trusted: bool) {
        let (rows, cols) = self.profile().map_or((3, 5), |x| (x.rows, x.cols));
        let accent = self.ws.settings.ui.accent.clone();
        let Some(folder) = self
            .current_page_mut()
            .and_then(|page| page.key_at_mut(folder_at.0, folder_at.1))
            .and_then(|key| key.folder.as_mut())
        else {
            return;
        };
        let Some(slot) = folder.free_folder_slot(rows, cols) else {
            self.log(false, "that folder is full");
            return;
        };
        let mut key = Key {
            title: title.into(),
            icon: icon.into(),
            color: if color.is_empty() { accent } else { color.into() },
            steps: vec![step],
            ..empty_key(slot.0, slot.1)
        };
        if trusted {
            key.approve();
        }
        folder.keys.push(key);
        self.selected = Some(folder_at);
    }

    /// Dropping on an empty slot makes a key. Dropping on a key adds a step to it. `trusted` says
    /// the person can vouch for the step, so it doesn't need a separate approval.
    #[allow(clippy::too_many_arguments)]
    fn add_step(&mut self, row: u32, col: u32, step: Step, title: &str, icon: &str, color: &str, trusted: bool) {
        let accent = self.ws.settings.ui.accent.clone();
        let mut added_to = None;
        if let Some(page) = self.current_page_mut() {
            if let Some(key) = page.key_at_mut(row, col) {
                let was_approved = key.is_approved();
                key.steps.push(step);
                added_to = Some((key.title.clone(), key.steps.len()));
                if was_approved && trusted {
                    key.approve();
                }
            } else {
                let mut key = Key {
                    title: title.into(),
                    icon: icon.into(),
                    color: if color.is_empty() { accent } else { color.into() },
                    steps: vec![step],
                    ..empty_key(row, col)
                };
                if trusted {
                    key.approve();
                }
                page.keys.push(key);
            }
        }
        if let Some((name, count)) = added_to {
            self.log(true, format!("added {title} to {name}, which now does {count} things"));
        }
        self.selected = Some((row, col));
        self.draft = None;
    }

    /// The biggest a title can be on a key this size, remembered until a lot have been asked about.
    fn fitted_label(&self, painter: &egui::Painter, title: &str, size: Vec2, layout: KeyLayout) -> FittedLabel {
        let room = size.y * (layout.label_bottom - layout.label_top);
        let wanted = (title.to_string(), size.x as u32, room as u32);
        let mut fitted = self.fitted_labels.borrow_mut();
        if fitted.len() > MAX_FITTED_LABELS {
            fitted.clear();
        }
        fitted
            .entry(wanted)
            .or_insert_with(|| {
                let measure = |text: &str, size: f32| painter.layout_no_wrap(text.to_string(), FontId::proportional(size), Color32::WHITE).size().x;
                fit_label(title, size.x * 0.88, room, (size.y * 0.09, size.y), &measure)
            })
            .clone()
    }

    /// The picture for a drawn icon, or nothing if this icon is a character.
    fn drawn_icon(&self, ctx: &egui::Context, icon: &str) -> Option<egui::TextureId> {
        icon.starts_with(DRAWN_ICON_PREFIX).then(|| self.texture_for(ctx, icon).map(|texture| texture.id())).flatten()
    }

    fn texture_for(&self, ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
        let mut textures = self.textures.borrow_mut();
        textures
            .entry(path.to_string())
            .or_insert_with(|| {
                let image = match crate::icon::named(path, 128) {
                    Some(drawn) => drawn,
                    None => crate::icon::open_capped(path)?,
                };
                let size = [image.width() as usize, image.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                Some(ctx.load_texture(path, color, egui::TextureOptions::LINEAR))
            })
            .clone()
    }

    fn draw_back_key(&self, ui: &Ui, rect: egui::Rect, response: &egui::Response, p: Palette) {
        let radius = CornerRadius::same((rect.width() * 0.14) as u8);
        let fill = parse_color(BACK_COLOR).unwrap_or(p.raised);
        let fill = if response.is_pointer_button_down_on() { fill.gamma_multiply(0.75) } else { fill };
        let painter = ui.painter_at(rect.expand(3.0));
        painter.rect_filled(rect, radius, fill);
        painter.text(rect.center() - Vec2::new(0.0, rect.height() * 0.1), Align2::CENTER_CENTER, "⬅", FontId::proportional(rect.width() * 0.34), Color32::WHITE);
        painter.text(rect.center_bottom() - Vec2::new(0.0, rect.height() * 0.17), Align2::CENTER_CENTER, "Back", FontId::proportional((rect.width() * 0.125).clamp(9.0, 13.0)), Color32::WHITE);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_key(&self, ui: &mut Ui, rect: egui::Rect, row: u32, col: u32, response: &egui::Response, p: Palette, key: Option<&Key>) {
        let selected = !self.use_mode && self.selected == Some((row, col));
        let id = self.key_id(row, col);
        let radius = CornerRadius::same((rect.width() * 0.14) as u8);
        let hovering_drop = response.dnd_hover_payload::<Drag>().is_some();
        let painter = ui.painter_at(rect.expand(3.0));
        let dragging = egui::DragAndDrop::payload::<Drag>(ui.ctx());
        let slot = (row, col);
        let hint = |has_key: bool, is_folder: bool| {
            dragging.as_ref().and_then(|payload| drop_hint(payload, slot, has_key, is_folder))
        };
        // What a drop would do, spelled out on the key under the pointer.
        let show_hint = |painter: &egui::Painter, text: &str| {
            painter.rect_filled(rect, radius, Color32::from_black_alpha(235));
            painter.rect_stroke(rect.expand(1.0), radius, Stroke::new(3.0, p.accent), StrokeKind::Outside);
            let font = FontId::proportional((rect.width() * 0.14).clamp(10.0, 15.0));
            let mut job = egui::text::LayoutJob::simple(text.to_string(), font, Color32::WHITE, rect.width() - 12.0);
            job.halign = egui::Align::Center;
            let galley = painter.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            let at = egui::pos2(rect.center().x, rect.center().y - galley.size().y / 2.0);
            painter.galley(at, galley, Color32::WHITE);
        };
        let Some(key) = key else {
            let stroke = if hovering_drop || selected { Stroke::new(2.0, p.accent) } else { Stroke::new(1.0, p.border) };
            let fill = if response.hovered() && !self.use_mode { p.raised } else { p.panel };
            if self.use_mode {
                painter.rect_filled(rect, radius, p.bg);
                painter.rect_stroke(rect, radius, Stroke::new(1.0, p.panel), StrokeKind::Inside);
                return;
            }
            painter.rect(rect, radius, fill, stroke, StrokeKind::Inside);
            match hint(false, false) {
                Some(text) if hovering_drop => show_hint(&painter, text),
                _ if response.hovered() => {
                    painter.text(rect.center(), Align2::CENTER_CENTER, "+", FontId::proportional(rect.width() * 0.25), p.muted);
                }
                _ => {}
            }
            return;
        };

        let KeyLook { title, icon, icon_color, color: color_text } = self.key_look(key);
        let fill = parse_color(&color_text).unwrap_or(p.raised);
        let fill = if response.is_pointer_button_down_on() { fill.gamma_multiply(0.75) } else { fill };
        let mut text = parse_color(&key.text_color).unwrap_or_else(|| readable_on(fill));
        painter.rect_filled(rect, radius, fill);

        let has_title = !title.is_empty();
        let background = self.background_path(&key.background);
        let mut on_picture = false;
        if !background.is_empty()
            && let Some(texture) = self.texture_for(ui.ctx(), &background)
        {
            let tint = if response.is_pointer_button_down_on() { Color32::from_gray(190) } else { Color32::WHITE };
            egui::Image::from_texture((texture.id(), rect.size())).corner_radius(radius).tint(tint).paint_at(ui, rect);
            on_picture = true;
        }
        let position = TitlePosition::from_setting(&key.title_position);
        let layout = position.layout(!on_picture && !icon.is_empty());
        let icon_size = rect.width() * if has_title { 0.34 } else { 0.46 };
        let icon_y = rect.top() + rect.height() * if has_title { layout.icon_center } else { 0.5 };
        // A title on top of the icon needs it to stand back.
        let icon_tint = if has_title && layout.dim_icon { Color32::WHITE.gamma_multiply(0.35) } else { Color32::WHITE };
        if on_picture {
            // The picture is the key's look, so the small icon stays out of its way.
        } else if icon.starts_with(DRAWN_ICON_PREFIX) {
            if let Some(texture) = self.drawn_icon(ui.ctx(), &icon) {
                let image_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, icon_y), Vec2::splat(icon_size * 1.5));
                painter.image(texture, image_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), icon_tint);
            }
        } else if icon.contains(['/', '\\']) || icon.ends_with(".png") || icon.ends_with(".jpg") {
            if let Some(texture) = self.texture_for(ui.ctx(), &icon) {
                let image_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, icon_y), Vec2::splat(icon_size * 1.3));
                painter.image(texture.id(), image_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), icon_tint);
            }
        } else if !icon.is_empty() {
            painter.text(egui::pos2(rect.center().x, icon_y), Align2::CENTER_CENTER, &icon, FontId::proportional(icon_size), parse_color(&icon_color).unwrap_or(text).gamma_multiply(if has_title && layout.dim_icon { 0.35 } else { 1.0 }));
        }
        if has_title {
            if on_picture && key.text_color.is_empty() {
                text = Color32::WHITE;
            }
            let fitted = self.fitted_label(&painter, &title, rect.size(), layout);
            let line_height = fitted.size * LINE_HEIGHT;
            let total = line_height * fitted.lines.len() as f32;
            let first = rect.top() + position.first_line_at(layout, rect.height(), total);
            if on_picture {
                // A dark band keeps the title readable on any picture.
                let band = egui::Rect::from_x_y_ranges(rect.x_range(), (first - 4.0).max(rect.top())..=(first + total + 4.0).min(rect.bottom()));
                let (top, bottom) = (band.top() <= rect.top(), band.bottom() >= rect.bottom());
                let corner = |round: bool| if round { radius.se } else { 0 };
                let shape = CornerRadius { nw: corner(top), ne: corner(top), sw: corner(bottom), se: corner(bottom) };
                painter.rect_filled(band, shape, Color32::from_black_alpha(130));
            }
            for (index, line) in fitted.lines.iter().enumerate() {
                let center = egui::pos2(rect.center().x, first + line_height * (index as f32 + 0.5));
                painter.text(center, Align2::CENTER_CENTER, line, FontId::proportional(fitted.size), text);
            }
        }
        if key.folder.is_some() && key.icon != FOLDER_ICON {
            painter.text(rect.left_top() + Vec2::new(11.0, 11.0), Align2::CENTER_CENTER, FOLDER_ICON, FontId::proportional(12.0), text);
        }
        // How many things a key does, on its face.
        if key.steps.len() > 1 {
            let center = rect.left_top() + Vec2::new(13.0, 13.0);
            painter.circle_filled(center, 9.0, Color32::from_black_alpha(150));
            painter.text(center, Align2::CENTER_CENTER, key.steps.len().to_string(), FontId::proportional(11.5), Color32::WHITE);
        }

        // A key that wouldn't run says so right on its face.
        let blocked = !self.engine.check_key(key).is_empty();
        if blocked {
            let center = rect.right_top() + Vec2::new(-11.0, 11.0);
            painter.circle_filled(center, 8.0, p.warn);
            painter.text(center, Align2::CENTER_CENTER, "!", FontId::proportional(12.0), Color32::BLACK);
        }
        if self.running.contains(&id) {
            painter.rect_filled(rect, radius, Color32::from_black_alpha(90));
            painter.text(rect.center(), Align2::CENTER_CENTER, "…", FontId::proportional(rect.width() * 0.3), Color32::WHITE);
            ui.ctx().request_repaint();
        }
        if let Some((at, ok)) = self.flashes.get(&id).copied() {
            let age = at.elapsed();
            if age < FLASH {
                let strength = 1.0 - age.as_secs_f32() / FLASH.as_secs_f32();
                let color = if ok { p.ok } else { p.bad };
                painter.rect_stroke(rect.expand(1.0), radius, Stroke::new(3.0, color.gamma_multiply(strength)), StrokeKind::Outside);
                ui.ctx().request_repaint();
            }
        }
        if selected {
            painter.rect_stroke(rect.expand(1.0), radius, Stroke::new(2.0, p.accent), StrokeKind::Outside);
        }
        if let Some(text) = hint(true, key.folder.is_some()) {
            if hovering_drop {
                show_hint(&painter, text);
            } else if matches!(dragging.as_deref(), Some(Drag::Action(_) | Drag::Preset(_))) {
                // While an action is on the move, every key it could join says so.
                let center = rect.right_top() + Vec2::new(-14.0, 14.0);
                painter.circle_filled(center, 10.0, p.accent);
                painter.text(center, Align2::CENTER_CENTER, "+", FontId::proportional(15.0), Color32::WHITE);
            }
        }
    }

    fn library(&mut self, ui: &mut Ui, p: Palette) {
        if self.use_mode {
            self.library_in_use_mode(ui, p);
            return;
        }
        ui.label(RichText::new("Actions").strong().size(15.0));
        ui.label(RichText::new("Drag one onto an empty key to make a new key, or onto a key that already does something to add another action to it.").small().color(p.muted));
        let heading = ui.label(RichText::new("Search actions").small().color(p.muted));
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("🔍 Search").desired_width(f32::INFINITY)).labelled_by(heading.id);
        ui.add_space(4.0);
        let search = self.search.to_lowercase();
        let infos = self.engine.plugin_infos();
        let presets = self.ws.settings.presets.clone();
        let searching = !search.is_empty();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if "folder".contains(&search) && self.action_group(ui, p, "organize", "ORGANIZE", 1, searching) {
                library_item(ui, p, FOLDER_ICON, None, "Folder", "A separate screen of keys. Drop keys and actions on it to fill it.", Drag::Folder);
                ui.add_space(6.0);
            }
            let matching: Vec<&Preset> = presets.iter().filter(|x| x.name.to_lowercase().contains(&search)).collect();
            if !matching.is_empty() && self.action_group(ui, p, "presets", "MY PRESETS", matching.len(), searching) {
                for preset in matching {
                    let drag = Drag::Preset(preset.clone());
                    let drawn = self.drawn_icon(ui.ctx(), &preset.icon);
                    library_item(ui, p, &preset.icon, drawn, &preset.name, "From your config", drag);
                }
                ui.add_space(6.0);
            }
            for info in infos {
                let actions: Vec<&ActionSpec> = info
                    .actions
                    .iter()
                    .filter(|a| !a.hidden)
                    .filter(|a| a.name.to_lowercase().contains(&search) || info.name.to_lowercase().contains(&search))
                    .collect();
                if actions.is_empty() || !self.action_group(ui, p, &info.id, &info.name.to_uppercase(), actions.len(), searching) {
                    continue;
                }
                for action in actions {
                    let drag = Drag::Action(format!("{}/{}", info.id, action.id));
                    let drawn = self.drawn_icon(ui.ctx(), &action.icon);
                    library_item(ui, p, &action.icon, drawn, &action.name, &action.description, drag);
                }
                ui.add_space(6.0);
            }
        });
    }

    /// A group's heading in the action list: click it to fold the group away or open it again.
    /// Says whether the group is open. A search looks through folded groups too, so nothing it
    /// finds is hidden.
    fn action_group(&mut self, ui: &mut Ui, p: Palette, id: &str, name: &str, count: usize, searching: bool) -> bool {
        let collapsed = self.ws.settings.ui.collapsed_groups.iter().any(|group| group == id);
        let open = searching || !collapsed;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), GROUP_HEADING_HEIGHT), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
        let color = if response.hovered() { p.text } else { p.muted };
        if response.hovered() {
            ui.painter().rect_filled(rect, 4, p.raised);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // Points down when open, and right when folded.
        let center = egui::pos2(rect.left() + 9.0, rect.center().y);
        let points = if open {
            vec![center + Vec2::new(-4.0, -2.5), center + Vec2::new(4.0, -2.5), center + Vec2::new(0.0, 3.5)]
        } else {
            vec![center + Vec2::new(-2.5, -4.0), center + Vec2::new(3.5, 0.0), center + Vec2::new(-2.5, 4.0)]
        };
        ui.painter().add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        let small = FontId::proportional(11.5);
        ui.painter().text(rect.left_center() + Vec2::new(19.0, 0.0), Align2::LEFT_CENTER, name, small.clone(), color);
        if !open {
            ui.painter().text(rect.right_center() - Vec2::new(6.0, 0.0), Align2::RIGHT_CENTER, count.to_string(), small, color);
        }
        if response.clicked() && !searching {
            let groups = &mut self.ws.settings.ui.collapsed_groups;
            if collapsed {
                groups.retain(|group| group != id);
            } else {
                groups.push(id.to_string());
            }
            self.mark_settings_dirty();
        }
        open
    }

    fn inspector(&mut self, ui: &mut Ui, p: Palette) {
        if self.use_mode {
            self.inspector_in_use_mode(ui, p);
            return;
        }
        let Some((row, col)) = self.selected else {
            self.activity_only(ui, p);
            return;
        };
        let stored = self.current_page().and_then(|page| page.key_at(row, col)).cloned();
        let Some(original) = self.draft.clone().or(stored) else {
            ui.columns(2, |columns| {
                columns[0].vertical_centered(|ui| {
                    ui.add_space(70.0);
                    ui.label(RichText::new("Empty key").strong());
                    ui.label(RichText::new("Drag an action here from the list on the right.").color(p.muted));
                });
                self.activity(&mut columns[1], p);
            });
            return;
        };
        let was_approved = original.is_approved();
        let mut key = original.clone();
        let is_folder = key.folder.is_some();
        let in_folder_count = key.folder.as_ref().map_or(0, |folder| folder.keys.len());
        let mut new_secret: Option<SecretTarget> = None;
        let mut choose_picture = false;
        let mut remove = false;
        let mut dropped_step: Option<Arc<Drag>> = None;
        let mut picked_action: Option<String> = None;
        let mut preview = None;
        let mut press = false;
        let mut approve = false;
        // A step came from a file, so the key's approval doesn't carry over to it.
        let mut untrusted = false;
        let mut allow_site: Option<(String, String)> = None;
        let mut favorites = self.ws.settings.ui.favorite_colors.clone();
        let mut browse: Option<Browse> = None;
        let lantern = self.drawn_icon(ui.ctx(), SLEEP_ICON);

        let total = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.allocate_ui(Vec2::new(total * 0.26, ui.available_height()), |ui| {
                ui.vertical(|ui| {
                    ui.set_width(total * 0.26);
                    key_look_form(ui, &mut key, &mut favorites, lantern, p, &mut choose_picture, &mut browse);
                });
            });
            ui.separator();
            ui.allocate_ui(Vec2::new(total * 0.46, ui.available_height()), |ui| {
                ui.vertical(|ui| {
                    ui.set_width(total * 0.46);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("What it does").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let asking = self.confirm_remove == Some((row, col));
                            let label = match (is_folder && in_folder_count > 0, asking) {
                                (true, true) => format!("Click again: this removes {in_folder_count} key(s) too"),
                                (true, false) => "Remove folder".to_string(),
                                (false, _) => "Remove key".to_string(),
                            };
                            if ui.button(label).clicked() {
                                if is_folder && in_folder_count > 0 && !asking {
                                    self.confirm_remove = Some((row, col));
                                } else {
                                    remove = true;
                                }
                            }
                            let main = if is_folder { "📂 Open folder" } else { "▶ Press" };
                            if ui.add(egui::Button::new(RichText::new(main).color(Color32::WHITE)).fill(p.accent)).clicked() {
                                press = true;
                            }
                        });
                    });
                    if is_folder {
                        ui.label(format!("Opens its own screen, with {in_folder_count} key(s) in it."));
                        ui.label(RichText::new("Drop keys and actions on this key to put them inside. Its first slot is always the way back.").small().color(p.muted));
                    } else {
                        // In a slot of its own: warnings come and go while typing, and if that
                        // moved the boxes below along, the one being typed in would lose the keyboard.
                        ui.scope(|ui| self.key_notices(ui, &key, p, &mut approve, &mut allow_site));
                        if key.steps.len() > 1 {
                            ui.label(RichText::new("Runs these in order, and stops if one fails.").small().color(p.muted));
                        }
                        let drawn_step = key.steps.iter().find(|step| self.engine.spec(&step.action).is_some_and(|spec| spec.previews));
                        if let Some(step) = drawn_step
                            && ui.small_button("Preview the animation").on_hover_text("Plays it now. Click any key to stop.").clicked()
                            && let Some((plugin, _)) = step.action.split_once('/')
                        {
                            preview = Some((plugin.to_string(), step.settings.clone()));
                        }
                        egui::ScrollArea::vertical().id_salt("steps").auto_shrink(false).show(ui, |ui| {
                            self.steps_form(ui, &mut key, &mut favorites, p, &mut new_secret, &mut browse);
                            dropped_step = add_action_zone(ui, p);
                            self.add_action_menu(ui, &mut picked_action);
                        });
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| self.activity(ui, p));
        });

        if favorites != self.ws.settings.ui.favorite_colors {
            self.ws.settings.ui.favorite_colors = favorites;
            self.mark_settings_dirty();
        }
        if let Some((plugin, site)) = allow_site {
            match Grants::allow_host(&self.ws.paths, &plugin, &site) {
                Ok(()) => {
                    self.grants = Grants::load(&self.ws.paths);
                    let _ = pyroporch_core::policy::Pending::remove_host(&self.ws.paths, &plugin, &site);
                    self.pending = pyroporch_core::policy::Pending::load(&self.ws.paths);
                    self.log(true, format!("allowed {site}"));
                }
                Err(error) => self.log(false, error),
            }
        }
        if let Some(target) = new_secret {
            let hosts = self.engine.sites_called(&key).into_iter().map(|(_, site)| site).collect::<Vec<_>>().join(", ");
            self.secret_dialog = Some(SecretDialog {
                hosts,
                target: Some(target),
                ..SecretDialog::default()
            });
        }
        if remove {
            // What's on screen may be an edit in progress, from before the slot changed. A folder
            // with keys in it always gets its second click, going by what's really there.
            let holding = self.current_page().and_then(|page| page.key_at(row, col)).and_then(|key| key.folder.as_ref()).is_some_and(|folder| !folder.keys.is_empty());
            if holding && self.confirm_remove != Some((row, col)) {
                self.draft = None;
                self.confirm_remove = Some((row, col));
                return;
            }
            self.confirm_remove = None;
            self.remove_key(row, col);
            return;
        }
        if choose_picture {
            self.choose_picture((row, col));
        }
        if let Some((plugin, settings)) = preview {
            let ctx = ui.ctx().clone();
            self.preview_saver(&ctx, plugin, settings);
        }
        if let Some(action) = picked_action {
            untrusted |= self.add_to_draft(&mut key, Drag::Action(action));
        }
        if let Some(payload) = dropped_step {
            untrusted |= self.add_to_draft(&mut key, (*payload).clone());
        }
        // After the form is drawn, because this waits for the system's dialog to close.
        if let Some(browse) = browse
            && let Some(path) = (self.pick_path)(browse.pick)
        {
            let path = path.to_string_lossy().into_owned();
            match browse.setting {
                Some((step, setting)) => {
                    if let Some(step) = key.steps.get_mut(step) {
                        step.settings.insert(setting, Value::String(path));
                    }
                }
                None => key.icon = path,
            }
        }
        if approve {
            key.approve();
        } else if untrusted {
            // The step came from a file, so the person hasn't seen it. The approval it had
            // doesn't carry over to it.
            key.approved.clear();
        } else if key != original && was_approved {
            // Editing here is the person's own choice, so an approved key stays approved.
            key.approve();
        }
        if key != original {
            self.commit_key(key);
        }
        if press {
            let id = self.key_id(row, col);
            self.press(ui.ctx(), id, Trigger::Press);
        }
    }

    /// Adds an action to the key being edited. Only actions can go on a key this way.
    /// Says whether the step came from a file, so the key's approval doesn't carry over to it.
    fn add_to_draft(&mut self, key: &mut Key, payload: Drag) -> bool {
        let (step, trusted) = match payload {
            Drag::Action(action) => {
                let Some(spec) = self.engine.spec(&action) else {
                    return false;
                };
                let settings = default_settings(&spec);
                (Step { action, settings, delay_ms: 0 }, true)
            }
            // A preset's settings come from a file, so the person hasn't seen them yet.
            Drag::Preset(preset) => (Step { action: preset.action, settings: preset.settings, delay_ms: 0 }, false),
            Drag::Folder | Drag::Key(..) => {
                self.log(false, "only actions can be added here");
                return false;
            }
        };
        let was_approved = key.is_approved();
        key.steps.push(step);
        if was_approved && trusted {
            key.approve();
        } else {
            // Not vouched for, so the approval carried over from before the edit doesn't apply.
            key.approved.clear();
        }
        !trusted
    }

    /// Lets actions be added without dragging.
    fn add_action_menu(&self, ui: &mut Ui, picked: &mut Option<String>) {
        ui.menu_button("+ Add another action", |ui| {
            for info in self.engine.plugin_infos() {
                let actions: Vec<&ActionSpec> = info.actions.iter().filter(|a| !a.hidden).collect();
                if actions.is_empty() {
                    continue;
                }
                ui.label(RichText::new(info.name.to_uppercase()).small());
                for action in actions {
                    if ui.button(&action.name).clicked() {
                        *picked = Some(format!("{}/{}", info.id, action.id));
                        ui.close();
                    }
                }
            }
        });
    }

    /// Stores an edit, unless it would write a credential into the config in plain text.
    fn commit_key(&mut self, key: Key) {
        let plain = key
            .steps
            .iter()
            .flat_map(|step| self.engine.check_step(step))
            .any(|block| matches!(block, Block::PlainCredential(_)));
        if plain {
            self.draft = Some(key);
            return;
        }
        self.draft = None;
        if let Some(page) = self.current_page_mut() {
            page.put(key);
        }
        self.mark_profile_dirty();
    }

    /// What's stopping this key, with the button to fix it where there is one.
    fn key_notices(&self, ui: &mut Ui, key: &Key, p: Palette, approve: &mut bool, allow_site: &mut Option<(String, String)>) {
        for block in self.engine.check_key(key) {
            match &block {
                Block::NeedsApproval => {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(p.warn, "⚠ Not approved: this key acts on your computer. Check its settings below, then");
                        if ui.small_button("Approve").clicked() {
                            *approve = true;
                        }
                    });
                }
                Block::PlainCredential(text) => {
                    ui.colored_label(p.bad, format!("⚠ Not saved. {text}"));
                }
                other => {
                    ui.colored_label(p.warn, format!("⚠ {other}"));
                }
            }
        }
        for (plugin, site) in self.engine.sites_called(key) {
            let allowed = self.grants.allows_host(&plugin, &site);
            if !allowed {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(p.warn, format!("⚠ This key can't contact {site} until you allow it."));
                    if ui.small_button(format!("Allow {site}")).clicked() {
                        *allow_site = Some((plugin.clone(), site.clone()));
                    }
                });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn steps_form(&self, ui: &mut Ui, key: &mut Key, favorites: &mut Vec<String>, p: Palette, new_secret: &mut Option<SecretTarget>, browse: &mut Option<Browse>) {
        if key.steps.is_empty() {
            ui.label(RichText::new("Nothing yet. Drag an action onto this key.").color(p.muted));
        }
        let names: Vec<String> = self
            .engine
            .vault
            .lock()
            .map_or_else(|_| Vec::new(), |vault| vault.list().into_iter().map(|s| s.name).collect());
        let numbered = key.steps.len() > 1;
        let mut remove_step = None;
        let mut move_up = None;
        // The key the secret would go on, so it lands there and not on another one.
        let id = self.key_id(key.row, key.col);
        for (index, step) in key.steps.iter_mut().enumerate() {
            let Some(spec) = self.engine.spec(&step.action) else {
                ui.colored_label(p.warn, format!("⚠ {} isn't installed", step.action));
                if ui.small_button("Remove this step").clicked() {
                    remove_step = Some(index);
                }
                continue;
            };
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    let glyph = if spec.icon.starts_with(DRAWN_ICON_PREFIX) { "" } else { spec.icon.as_str() };
                    let number = if numbered { format!("{}.  ", index + 1) } else { String::new() };
                    ui.label(RichText::new(format!("{number}{glyph} {}", spec.name)).strong());
                    if spec.risk == Risk::High {
                        ui.label(RichText::new("needs approval").small().color(p.warn));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Remove this step").clicked() {
                            remove_step = Some(index);
                        }
                        if index > 0 && ui.small_button("⬆").on_hover_text("Do this earlier").clicked() {
                            move_up = Some(index);
                        }
                    });
                });
                if !spec.description.is_empty() {
                    ui.label(RichText::new(&spec.description).small().color(p.muted));
                }
                egui::Grid::new("fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for field in &spec.fields {
                        if let Some((other, wanted)) = &field.visible_when
                            && step.settings.get(other).and_then(Value::as_str) != Some(wanted.as_str())
                        {
                            continue;
                        }
                        if hidden_by_other_field(field, &spec.fields, &step.settings) {
                            continue;
                        }
                        let label = ui.label(&field.label).on_hover_text(&field.help);
                        field_form(ui, field, label.id, &mut step.settings, &names, index, &id, new_secret, favorites, p, browse);
                        ui.end_row();
                    }
                    if numbered && index > 0 {
                        let label = ui.label("Wait first");
                        ui.add(egui::DragValue::new(&mut step.delay_ms).range(0..=MAX_STEP_WAIT_MS).speed(50).suffix(" ms"))
                            .labelled_by(label.id)
                            .on_hover_text("How long to wait after the action before this one");
                        ui.end_row();
                    }
                });
            });
            ui.separator();
        }
        if let Some(index) = remove_step {
            key.steps.remove(index);
        }
        if let Some(index) = move_up {
            key.steps.swap(index, index - 1);
        }
    }

    fn activity_only(&mut self, ui: &mut Ui, p: Palette) {
        ui.columns(2, |columns| {
            columns[0].vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.label(RichText::new("Pick a key to set it up").strong());
                ui.label(RichText::new("Or drag an action from the list onto an empty key.").color(p.muted));
            });
            self.activity(&mut columns[1], p);
        });
    }

    fn activity(&mut self, ui: &mut Ui, p: Palette) {
        ui.label(RichText::new("Activity").strong());
        egui::ScrollArea::vertical().id_salt("activity").auto_shrink(false).show(ui, |ui| {
            if self.activity.is_empty() {
                ui.label(RichText::new("Nothing has happened yet. Press a key to try it.").color(p.muted));
            }
            for line in &self.activity {
                let color = if line.ok { p.muted } else { p.bad };
                ui.label(RichText::new(&line.text).color(color).small());
            }
        });
    }
}

fn library_item(ui: &mut Ui, p: Palette, icon: &str, drawn: Option<egui::TextureId>, name: &str, description: &str, drag: Drag) {
    let id = ui.make_persistent_id(("library", name, description));
    let response = ui
        .dnd_drag_source(id, drag, |ui| {
            egui::Frame::new().fill(p.raised).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    match drawn {
                        Some(texture) => {
                            ui.add(egui::Image::from_texture((texture, Vec2::splat(18.0))));
                        }
                        None => {
                            ui.label(RichText::new(icon).size(15.0));
                        }
                    }
                    ui.label(name);
                });
            });
        })
        .response;
    if !description.is_empty() {
        response.on_hover_text(description);
    }
}

/// What a browse button is looking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// On a Mac this includes apps, which the system treats as files here.
    FileOrApp,
    Folder,
    Icon,
}

/// A browse button that was pressed, and where the answer goes.
struct Browse {
    pick: Pick,
    /// The step and setting to fill in, or the key's icon if nothing's here.
    setting: Option<(usize, String)>,
}

pub fn ask_system_for_path(pick: Pick) -> Option<std::path::PathBuf> {
    let dialog = rfd::FileDialog::new();
    match pick {
        Pick::FileOrApp => dialog.set_title("Choose an app or file").pick_file(),
        Pick::Folder => dialog.set_title("Choose a folder").pick_folder(),
        Pick::Icon => dialog.set_title("Choose an icon").add_filter("Pictures", &["png", "jpg", "jpeg"]).pick_file(),
    }
}

impl App {
    /// Asks for a picture for a key.
    fn choose_picture(&mut self, slot: (u32, u32)) {
        let picked = rfd::FileDialog::new().set_title("Choose a picture for this key").add_filter("Pictures", &["png", "jpg", "jpeg"]).pick_file();
        if let Some(path) = picked {
            self.use_picture(&path, slot);
        }
    }

    /// A picture dragged onto the window goes on the selected key.
    fn dropped_picture(&mut self, ui: &Ui) {
        if self.use_mode {
            return;
        }
        let dropped = ui.input(|input| input.raw.dropped_files.first().map(|file| file.path().to_path_buf()));
        if let (Some(path), Some(slot)) = (dropped, self.selected)
            && self.current_page().is_some_and(|page| page.key_at(slot.0, slot.1).is_some())
        {
            self.use_picture(&path, slot);
        }
    }

    /// Uses a picture straight away if it fits a key, and opens the fitting screen if it doesn't.
    pub fn use_picture(&mut self, path: &std::path::Path, slot: (u32, u32)) {
        match open_picture(&self.ws.paths, path) {
            Ok(Opened::Ready(name)) => self.set_background(slot, name),
            Ok(Opened::Fit(picture)) => self.crop = Some(CropDialog::new(&self.ctx_for_crop, picture, slot)),
            Err(error) => self.log(false, error),
        }
    }

    pub(super) fn set_background(&mut self, slot: (u32, u32), name: String) {
        if let Some(key) = self.current_page_mut().and_then(|page| page.key_at_mut(slot.0, slot.1)) {
            key.background = name.clone();
        }
        if let Some(draft) = &mut self.draft {
            draft.background = name;
        }
        self.mark_profile_dirty();
    }

    pub fn crop_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.crop.take() else {
            return;
        };
        let p = self.palette();
        let key_color = self
            .current_page()
            .and_then(|page| page.key_at(dialog.slot.0, dialog.slot.1))
            .and_then(|key| parse_color(&key.color))
            .unwrap_or(p.raised);
        match dialog.show(ctx, p.accent, key_color, p.muted) {
            CropResult::Open => self.crop = Some(dialog),
            CropResult::Cancelled => {}
            CropResult::Done(picture) => {
                let stored = encode_png(&picture).and_then(|png| store_image(&self.ws.paths, &png));
                match stored {
                    Ok(name) => self.set_background(dialog.slot, name),
                    Err(error) => {
                        dialog.error = error;
                        self.crop = Some(dialog);
                    }
                }
            }
        }
    }

    /// A sample for checking the fitting screen in a picture.
    pub fn start_demo_crop(&mut self) {
        let picture = image::RgbaImage::from_fn(900, 500, |x, y| {
            let ember = (x * 255 / 900) as u8;
            image::Rgba([230, 110u8.saturating_add(ember / 3), 60u8.saturating_add((y / 4) as u8), 255])
        });
        self.crop = Some(CropDialog::new(&self.ctx_for_crop, picture, (0, 1)));
    }
}

/// A place to drop another action onto the key being edited.
fn add_action_zone(ui: &mut Ui, p: Palette) -> Option<Arc<Drag>> {
    let frame = egui::Frame::new().fill(p.bg).stroke(Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(10);
    let (_, payload) = ui.dnd_drop_zone::<Drag, _>(frame, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Drop another action here to add it to this key").color(p.muted));
        });
    });
    payload
}

#[allow(clippy::too_many_arguments)]
fn key_look_form(ui: &mut Ui, key: &mut Key, favorites: &mut Vec<String>, lantern: Option<egui::TextureId>, p: Palette, choose_picture: &mut bool, browse: &mut Option<Browse>) {
    ui.label(RichText::new("Look").strong());
    egui::Grid::new("look").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        let label = ui.label("Title");
        ui.add(egui::TextEdit::singleline(&mut key.title).desired_width(ui.available_width() - 4.0)).labelled_by(label.id);
        ui.end_row();

        ui.label("Title sits at");
        ui.horizontal(|ui| {
            let current = TitlePosition::from_setting(&key.title_position);
            for (position, name) in [(TitlePosition::Top, "Top"), (TitlePosition::Middle, "Middle"), (TitlePosition::Bottom, "Bottom")] {
                if ui.selectable_label(current == position, name).clicked() {
                    key.title_position = position.setting().to_string();
                }
            }
        });
        ui.end_row();

        let label = ui.label("Icon");
        ui.horizontal(|ui| {
            // A drawn icon's name means nothing to anyone, so it shows as what it is.
            if let (true, Some(texture)) = (key.icon.starts_with(DRAWN_ICON_PREFIX), lantern) {
                ui.add(egui::Image::new((texture, Vec2::splat(ICON_BUTTON_SIZE)))).labelled_by(label.id);
            } else {
                ui.add(egui::TextEdit::singleline(&mut key.icon).desired_width(70.0).hint_text("🔗 or a file")).labelled_by(label.id);
            }
            ui.menu_button("Pick", |ui| {
                ui.set_max_width(210.0);
                ui.horizontal_wrapped(|ui| {
                    for choice in ICON_CHOICES {
                        if ui.button(choice).clicked() {
                            key.icon = choice.into();
                            ui.close();
                        }
                    }
                });
                if let Some(texture) = lantern {
                    let button = ui.add(egui::Button::image((texture, Vec2::splat(ICON_BUTTON_SIZE)))).on_hover_text("Lantern gone out");
                    button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Lantern gone out"));
                    if button.clicked() {
                        key.icon = SLEEP_ICON.into();
                        ui.close();
                    }
                }
            });
        });
        ui.end_row();

        ui.label("");
        if ui.small_button("Icon from a file…").on_hover_text("Browse for a PNG or JPEG to use as the icon").clicked() {
            *browse = Some(Browse { pick: Pick::Icon, setting: None });
        }
        ui.end_row();

        ui.label("Icon colour");
        color_button(ui, "Icon colour", &mut key.icon_color, favorites, p);
        ui.end_row();

        ui.label("Background");
        color_button(ui, "Background", &mut key.color, favorites, p);
        ui.end_row();

        ui.label("Picture");
        ui.horizontal(|ui| {
            let label = if key.background.is_empty() { "Choose…" } else { "Change…" };
            if ui.button(label).on_hover_text("A PNG or JPEG to fill the key. You can also drag one onto the window.").clicked() {
                *choose_picture = true;
            }
            if !key.background.is_empty() && ui.small_button("×").on_hover_text("Take the picture off").clicked() {
                key.background.clear();
            }
        });
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut key.toggle, "Two looks").on_hover_text("Each press flips between two looks");
        ui.end_row();

        if key.toggle {
            let label = ui.label("Other title");
            ui.add(egui::TextEdit::singleline(&mut key.alt_title).desired_width(ui.available_width() - 4.0)).labelled_by(label.id);
            ui.end_row();
            ui.label("Other colour");
            color_button(ui, "Other colour", &mut key.alt_color, favorites, p);
            ui.end_row();
        }
    });
}

/// A swatch that opens a colour picker, along with the colours kept for choosing again.
fn color_button(ui: &mut Ui, name: &str, value: &mut String, favorites: &mut Vec<String>, p: Palette) {
    let mut color = parse_color(value).unwrap_or(p.raised);
    let swatch = ui.add(egui::Button::new("     ").fill(color)).on_hover_text(name);
    swatch.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Choose {name}")));
    egui::Popup::menu(&swatch).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_max_width(260.0);
        if egui::color_picker::color_picker_color32(ui, &mut color, egui::color_picker::Alpha::Opaque) {
            *value = color_hex(color);
        }
        ui.horizontal(|ui| {
            hex_field(ui, value);
            if ui.small_button("Default").on_hover_text("Go back to no colour of its own").clicked() {
                value.clear();
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Favourites").small().color(p.muted));
            if parse_color(value).is_some() && ui.small_button("☆ Add").on_hover_text("Keep this colour for choosing again, on any key").clicked() {
                add_favorite(favorites, value);
            }
        });
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (index, favorite) in favorites.iter().enumerate() {
                let fill = parse_color(favorite).unwrap_or(p.raised);
                let button = ui.add(egui::Button::new("    ").fill(fill)).on_hover_text(format!("{favorite}. Right-click to remove it."));
                button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Favourite {favorite}")));
                if button.clicked() {
                    value.clone_from(favorite);
                }
                if button.secondary_clicked() {
                    remove = Some(index);
                }
            }
        });
        if let Some(index) = remove {
            favorites.remove(index);
        }
    });
}

/// For typing a colour in, which only takes effect once it's a whole colour.
fn hex_field(ui: &mut Ui, value: &mut String) {
    let id = ui.scope_id().with("hex");
    let mut text = ui.data(|data| data.get_temp::<String>(id)).unwrap_or_else(|| value.clone());
    let field = ui.add(egui::TextEdit::singleline(&mut text).id(id).desired_width(80.0).hint_text("#RRGGBB"));
    if field.changed() && parse_color(&text).is_some() {
        text.trim().clone_into(value);
    }
    // Text left half typed gives way to the colour, once the person has moved on.
    if field.has_focus() {
        ui.data_mut(|data| data.insert_temp(id, text));
    } else {
        ui.data_mut(|data| data.remove_temp::<String>(id));
    }
}

/// Keeps a colour for choosing again, once, however it's written.
pub fn add_favorite(favorites: &mut Vec<String>, color: &str) {
    let color = color.trim().to_uppercase();
    if !favorites.iter().any(|favorite| favorite.eq_ignore_ascii_case(&color)) {
        favorites.push(color);
    }
    let extra = favorites.len().saturating_sub(MAX_FAVORITE_COLORS);
    favorites.drain(..extra);
}

/// Draws the right editor for one setting, from the plugin's description of it.
#[allow(clippy::too_many_arguments)]
fn field_form(ui: &mut Ui, field: &Field, label: egui::Id, settings: &mut Map<String, Value>, secrets: &[String], step: usize, id: &KeyId, new_secret: &mut Option<SecretTarget>, favorites: &mut Vec<String>, p: Palette, browse: &mut Option<Browse>) {
    let key = field.key.as_str();
    let width = 320.0_f32.min(ui.available_width() - 4.0);
    match &field.kind {
        FieldKind::Text | FieldKind::Url | FieldKind::RequestUrl => {
            let mut text = settings.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(width).hint_text(&field.help)).labelled_by(label).changed() {
                settings.insert(key.into(), Value::String(text));
            }
        }
        FieldKind::MultiLine => {
            let mut text = settings.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            if ui.add(egui::TextEdit::multiline(&mut text).desired_width(width).desired_rows(2)).labelled_by(label).changed() {
                settings.insert(key.into(), Value::String(text));
            }
        }
        FieldKind::Number => {
            let mut number = settings.get(key).and_then(Value::as_f64).unwrap_or_else(|| field.default.as_f64().unwrap_or(0.0));
            if ui.add(egui::DragValue::new(&mut number).range(0.0..=86_400.0).clamp_existing_to_range(false)).labelled_by(label).on_hover_text(&field.help).changed() {
                settings.insert(key.into(), serde_json::json!(number));
            }
        }
        FieldKind::Bool => {
            let mut on = settings.get(key).and_then(Value::as_bool).unwrap_or(false);
            if ui.checkbox(&mut on, "").labelled_by(label).changed() {
                settings.insert(key.into(), Value::Bool(on));
            }
        }
        FieldKind::Select { options } => {
            let fallback = field.default.as_str().unwrap_or("").to_string();
            let current = settings.get(key).and_then(Value::as_str).map_or(fallback, str::to_string);
            egui::ComboBox::from_id_salt(key).selected_text(&current).show_ui(ui, |ui| {
                for option in options {
                    if ui.selectable_label(*option == current, option).clicked() {
                        settings.insert(key.into(), Value::String(option.clone()));
                    }
                }
            })
            .response
            .labelled_by(label);
        }
        FieldKind::Secret => {
            let current = settings.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            let shown = reference_name(&current).map_or_else(|| "None".to_string(), |name| format!("🔒 {name}"));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(key).selected_text(shown).width(160.0).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_empty(), "None").clicked() {
                        settings.remove(key);
                    }
                    for name in secrets {
                        let reference = pyroporch_sdk::secret_ref(name);
                        if ui.selectable_label(current == reference, format!("🔒 {name}")).clicked() {
                            settings.insert(key.into(), Value::String(reference));
                        }
                    }
                })
                .response
                .labelled_by(label);
                if ui.small_button("+ New").on_hover_text("Store a new key or password in the vault").clicked() {
                    *new_secret = Some(SecretTarget {
                        id: id.clone(),
                        step,
                        field: field.key.clone(),
                    });
                }
            });
        }
        FieldKind::Hotkey => {
            let mut text = settings.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            let accent = ui.visuals().selection.stroke.color;
            let id = ui.make_persistent_id(("hotkey", step, key));
            if hotkey::field(ui, id, label, &mut text, accent) {
                settings.insert(key.into(), Value::String(text));
            }
        }
        FieldKind::KeyValues => key_values_form(ui, key, label, settings, secrets, width),
        FieldKind::LocalPath { folders } => {
            let mut text = settings.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            ui.horizontal(|ui| {
                if ui.add(egui::TextEdit::singleline(&mut text).desired_width(width - if *folders { 170.0 } else { 90.0 }).hint_text(&field.help)).labelled_by(label).changed() {
                    settings.insert(key.into(), Value::String(text));
                }
                let mut browse_for = |pick| *browse = Some(Browse { pick, setting: Some((step, field.key.clone())) });
                if ui.button("File or app…").on_hover_text("Browse for an app, program or file").clicked() {
                    browse_for(Pick::FileOrApp);
                }
                if *folders && ui.button("Folder…").on_hover_text("Browse for a folder").clicked() {
                    browse_for(Pick::Folder);
                }
            });
        }
        FieldKind::Rules => rules_form(ui, key, label, settings, favorites, p),
    }
}

/// Whether a field says to stay out of the way, given another field's value or what that starts as.
fn hidden_by_other_field(field: &Field, fields: &[Field], settings: &Map<String, Value>) -> bool {
    let Some((other, value)) = &field.hidden_when else {
        return false;
    };
    let starts_as = || fields.iter().find(|f| f.key == *other).and_then(|f| f.default.as_str());
    settings.get(other).and_then(Value::as_str).or_else(starts_as) == Some(value.as_str())
}

fn rules_form(ui: &mut Ui, key: &str, label: egui::Id, settings: &mut Map<String, Value>, favorites: &mut Vec<String>, p: Palette) {
    let saved = settings.get(key).cloned().and_then(|rules| serde_json::from_value::<Vec<Rule>>(rules).ok());
    let mut rules = saved.clone().unwrap_or_default();
    let mut remove = None;
    ui.vertical(|ui| {
        // Rows that can't be read came from a file, and stay as they are until that's fixed.
        if saved.is_none() && settings.get(key).is_some() {
            ui.colored_label(p.warn, "⚠ These rules have a mistake. See the config file.");
            return;
        }
        for (index, rule) in rules.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("when").selected_text(rule.when.name()).width(96.0).show_ui(ui, |ui| {
                        for test in RuleTest::ALL {
                            ui.selectable_value(&mut rule.when, test, test.name());
                        }
                    })
                    .response
                    .labelled_by(label);
                    if rule.when != RuleTest::AnythingElse {
                        ui.add(egui::TextEdit::singleline(&mut rule.value).desired_width(76.0).hint_text("Value")).labelled_by(label);
                    }
                    ui.add(egui::TextEdit::singleline(&mut rule.icon).desired_width(30.0).hint_text("Icon")).labelled_by(label);
                    ui.menu_button("Pick", |ui| {
                        ui.set_max_width(210.0);
                        ui.horizontal_wrapped(|ui| {
                            for choice in STATUS_ICON_CHOICES.iter().chain(&ICON_CHOICES) {
                                if ui.button(*choice).clicked() {
                                    rule.icon = (*choice).into();
                                    ui.close();
                                }
                            }
                        });
                    });
                    color_button(ui, "Icon colour", &mut rule.icon_color, favorites, p);
                    color_button(ui, "Background", &mut rule.color, favorites, p);
                    if ui.small_button("×").on_hover_text("Remove this rule").clicked() {
                        remove = Some(index);
                    }
                });
            });
        }
        if ui.small_button("+ Add rule").clicked() {
            rules.push(Rule::default());
        }
    });
    if let Some(index) = remove {
        rules.remove(index);
    }
    if saved.as_ref().map_or(!rules.is_empty(), |saved| *saved != rules) {
        if rules.is_empty() {
            settings.remove(key);
        } else if let Ok(rules) = serde_json::to_value(&rules) {
            settings.insert(key.into(), rules);
        }
    }
}

fn key_values_form(ui: &mut Ui, key: &str, label: egui::Id, settings: &mut Map<String, Value>, secrets: &[String], width: f32) {
    let mut pairs: Vec<(String, String)> = settings
        .get(key)
        .and_then(Value::as_object)
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();
    let before = pairs.clone();
    let mut remove = None;
    ui.vertical(|ui| {
        for (index, (name, value)) in pairs.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(name).desired_width(width * 0.38).hint_text("Name")).labelled_by(label);
                ui.add(egui::TextEdit::singleline(value).desired_width(width * 0.38).hint_text("Value")).labelled_by(label);
                if !secrets.is_empty() {
                    ui.menu_button("🔒", |ui| {
                        for secret in secrets {
                            if ui.button(secret).clicked() {
                                *value = pyroporch_sdk::secret_ref(secret);
                                ui.close();
                            }
                        }
                    });
                }
                if ui.small_button("×").clicked() {
                    remove = Some(index);
                }
            });
            if !find_refs(value).is_empty() {
                ui.label(RichText::new("Filled in from the vault when sent").small());
            }
            });
        }
        if ui.small_button("+ Add").clicked() {
            // A free name, because two of the same name would become one, losing a value.
            let name = (pairs.len() + 1..).map(|n| format!("Header-{n}")).find(|name| pairs.iter().all(|(taken, _)| taken != name));
            pairs.extend(name.map(|name| (name, String::new())));
        }
    });
    if let Some(index) = remove {
        pairs.remove(index);
    }
    if pairs != before {
        let map: Map<String, Value> = pairs.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        settings.insert(key.into(), Value::Object(map));
    }
}
