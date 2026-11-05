//! The hotkey field: records what you press, or lets you choose from a list of every key.

use eframe::egui::{self, Event, Key, Modifiers, RichText, Ui};

/// The name of the Cmd/Windows/Super key here.
const META: &str = if cfg!(target_os = "macos") {
    "cmd"
} else if cfg!(target_os = "windows") {
    "win"
} else {
    "super"
};
const MODIFIERS: [&str; 4] = ["ctrl", "alt", "shift", META];
const ALL_META_NAMES: [&str; 6] = ["cmd", "command", "win", "windows", "super", "meta"];

const NAVIGATION: [&str; 8] = ["up", "down", "left", "right", "home", "end", "pageup", "pagedown"];
const EDITING: [&str; 6] = ["enter", "tab", "esc", "space", "backspace", "delete"];
const FUNCTION_KEYS: u32 = if cfg!(target_os = "macos") { 20 } else { 24 };
#[cfg(not(target_os = "macos"))]
const SYSTEM: [&str; 4] = ["insert", "printscreen", "pause", "numlock"];

fn is_modifier(name: &str) -> bool {
    matches!(name, "ctrl" | "control" | "alt" | "option" | "shift") || ALL_META_NAMES.contains(&name)
}

fn canonical(name: &str) -> &str {
    match name {
        "control" => "ctrl",
        "option" => "alt",
        name if ALL_META_NAMES.contains(&name) => META,
        name => name,
    }
}

/// Toggles a modifier, or sets the combination's main key. Keeps a fixed order.
pub fn add_key(combo: &str, name: &str) -> String {
    let parts: Vec<&str> = combo.split('+').map(str::trim).filter(|part| !part.is_empty()).map(canonical).collect();
    let name = canonical(name);
    let mut modifiers: Vec<&str> = MODIFIERS.iter().copied().filter(|m| parts.contains(m)).collect();
    let mut main = parts.iter().copied().find(|part| !is_modifier(part));
    if is_modifier(name) {
        if modifiers.contains(&name) {
            modifiers.retain(|m| *m != name);
        } else {
            modifiers = MODIFIERS.iter().copied().filter(|m| *m == name || modifiers.contains(m)).collect();
        }
    } else {
        main = Some(name);
    }
    modifiers.into_iter().chain(main).collect::<Vec<_>>().join("+")
}

/// The name of a key as a combination writes it, for the keys a keystroke can produce.
pub fn key_name(key: Key) -> Option<String> {
    use Key::*;
    let punctuation = match key {
        Comma => Some(","),
        Period => Some("."),
        Semicolon => Some(";"),
        Colon => Some(":"),
        Slash => Some("/"),
        Backslash => Some("\\"),
        OpenBracket => Some("["),
        CloseBracket => Some("]"),
        Backtick => Some("`"),
        Quote => Some("'"),
        Pipe => Some("|"),
        Equals => Some("="),
        Questionmark => Some("?"),
        Exclamationmark => Some("!"),
        _ => None,
    };
    if let Some(symbol) = punctuation {
        return Some(symbol.into());
    }
    Some(match key {
        Copy | Cut | Paste | OpenCurlyBracket | CloseCurlyBracket | ShiftLeft | ShiftRight | ControlLeft | ControlRight
        | AltLeft | AltRight | SuperLeft | SuperRight | MediaStop | LaunchMail | LaunchApp1 | LaunchApp2 | IntlBackslash
        | Menu | Fn | Eject | Help | Power | Sleep | Clear => return None,
        other if other.name().starts_with("Browser") => return None,
        MediaPlayPause => "playpause".into(),
        MediaTrackNext => "medianext".into(),
        MediaTrackPrevious => "mediaprevious".into(),
        AudioVolumeMute => "volumemute".into(),
        AudioVolumeDown => "volumedown".into(),
        AudioVolumeUp => "volumeup".into(),
        ArrowUp => "up".into(),
        ArrowDown => "down".into(),
        ArrowLeft => "left".into(),
        ArrowRight => "right".into(),
        Escape => "esc".into(),
        other => other.name().to_lowercase().replace(' ', ""),
    })
}

/// Watches keystrokes during recording.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Recorder {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    /// The Windows or Super key, held right now. Only key events report it there.
    meta_now: bool,
}

impl Recorder {
    fn modifiers_held(&self) -> Vec<&'static str> {
        [(self.ctrl, "ctrl"), (self.alt, "alt"), (self.shift, "shift"), (self.meta, META)]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect()
    }

    fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }

    /// One frame of input. `Some` when the recording is finished: a key with whatever modifiers
    /// are held, or the modifiers alone once they've all been let go.
    pub fn step(&mut self, now: Modifiers, events: &[Event]) -> Option<String> {
        for event in events {
            let copy_like = match event {
                Event::Copy => Some("c"),
                Event::Cut => Some("x"),
                Event::Paste(_) => Some("v"),
                _ => None,
            };
            if let Some(letter) = copy_like {
                // The system turned this into a clipboard command, so it was pressed with the
                // usual modifier.
                let modifier = if cfg!(target_os = "macos") { META } else { "ctrl" };
                return Some(format!("{modifier}+{letter}"));
            }
            let Event::Key { key, physical_key, pressed, repeat: false, modifiers } = event else {
                continue;
            };
            let physical = physical_key.unwrap_or(*key);
            match physical {
                Key::SuperLeft | Key::SuperRight => {
                    self.meta_now = *pressed;
                    continue;
                }
                Key::ShiftLeft | Key::ShiftRight | Key::ControlLeft | Key::ControlRight | Key::AltLeft | Key::AltRight => continue,
                _ => {}
            }
            if !pressed {
                continue;
            }
            // With shift held the key reads as its shifted symbol, so use the key's place.
            let key = if modifiers.shift { physical } else { *key };
            let Some(name) = key_name(key) else {
                continue;
            };
            let held = Self {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                meta: modifiers.mac_cmd || self.meta_now,
                meta_now: false,
            };
            let mut parts = held.modifiers_held();
            parts.push(&name);
            return Some(parts.join("+"));
        }
        let current = Self {
            ctrl: now.ctrl,
            alt: now.alt,
            shift: now.shift,
            meta: now.mac_cmd || self.meta_now,
            meta_now: false,
        };
        if !current.any() && self.any() {
            return Some(self.modifiers_held().join("+"));
        }
        self.ctrl |= current.ctrl;
        self.alt |= current.alt;
        self.shift |= current.shift;
        self.meta |= current.meta;
        None
    }

    /// What's been held so far, to show while recording.
    fn progress(&self) -> String {
        self.modifiers_held().join(" + ")
    }
}

fn pretty(combo: &str) -> String {
    combo.split('+').filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" + ")
}

/// The whole field. Returns true when the value changed.
pub fn field(ui: &mut Ui, id: egui::Id, label: egui::Id, value: &mut String, accent: egui::Color32) -> bool {
    let recorder_id = id.with("recording");
    let mut changed = false;
    let mut recorder = ui.ctx().data_mut(|data| data.get_temp::<Recorder>(recorder_id));
    let recording = recorder.is_some();
    ui.horizontal(|ui| {
        let shown = if let Some(active) = &recorder {
            let so_far = active.progress();
            if so_far.is_empty() { "Press keys now…".to_string() } else { format!("{so_far} + …") }
        } else if value.is_empty() {
            "No keys yet".to_string()
        } else {
            pretty(value)
        };
        let text = if recording { RichText::new(shown).color(accent) } else { RichText::new(shown).strong() };
        ui.add_sized([170.0, 24.0], egui::Label::new(text)).labelled_by(label);

        let button = if recording { "Stop" } else { "⏺ Record" };
        if ui.button(button).clicked() {
            recorder = if recording { None } else { Some(Recorder::default()) };
            if recorder.is_some() {
                ui.memory_mut(|memory| {
                    if let Some(focused) = memory.focused() {
                        memory.surrender_focus(focused);
                    }
                });
            }
        }
        let current = value.clone();
        ui.menu_button("Keys", |ui| {
            ui.set_max_width(290.0);
            let mut chip = |ui: &mut Ui, name: &str, label: String| {
                if ui.small_button(label).clicked() {
                    *value = add_key(value, name);
                    changed = true;
                }
            };
            ui.label(RichText::new("HOLD").small());
            ui.horizontal_wrapped(|ui| {
                for name in MODIFIERS {
                    chip(ui, name, name.to_string());
                }
            });
            ui.label(RichText::new("FUNCTION").small());
            ui.horizontal_wrapped(|ui| {
                for number in 1..=FUNCTION_KEYS {
                    chip(ui, &format!("f{number}"), format!("F{number}"));
                }
            });
            ui.label(RichText::new("MOVE").small());
            ui.horizontal_wrapped(|ui| {
                for name in NAVIGATION {
                    chip(ui, name, name.to_string());
                }
            });
            ui.label(RichText::new("EDIT").small());
            ui.horizontal_wrapped(|ui| {
                for name in EDITING {
                    chip(ui, name, name.to_string());
                }
            });
            #[cfg(not(target_os = "macos"))]
            {
                ui.label(RichText::new("SYSTEM").small());
                ui.horizontal_wrapped(|ui| {
                    for name in SYSTEM {
                        chip(ui, name, name.to_string());
                    }
                });
            }
        });
        if !current.is_empty() && ui.small_button("×").on_hover_text("Clear").clicked() {
            value.clear();
            changed = true;
        }
    });

    if let Some(active) = &mut recorder {
        let (modifiers, events) = ui.input(|input| (input.modifiers, input.events.clone()));
        if let Some(combo) = active.step(modifiers, &events) {
            *value = combo;
            changed = true;
            recorder = None;
        }
        ui.label(RichText::new("Shortcuts the system keeps for itself can't be recorded: use the Keys list.").small());
        ui.ctx().request_repaint();
    }
    ui.ctx().data_mut(|data| {
        if let Some(active) = recorder {
            data.insert_temp(recorder_id, active);
        } else {
            data.remove::<Recorder>(recorder_id);
        }
    });
    changed
}

// These tests use the desktop module, which is only built with that feature.
#[cfg(all(test, feature = "desktop-actions"))]
mod tests {
    use super::*;

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        }
    }

    #[test]
    fn records_a_combination_when_the_main_key_goes_down() {
        let mut recorder = Recorder::default();
        assert_eq!(recorder.step(ctrl_shift(), &[]), None);
        assert_eq!(recorder.progress(), "ctrl + shift");
        let finished = recorder.step(ctrl_shift(), &[press(Key::M, ctrl_shift())]);
        assert_eq!(finished.as_deref(), Some("ctrl+shift+m"));
    }

    #[test]
    fn records_modifiers_alone_once_they_are_released() {
        let mut recorder = Recorder::default();
        let alt = Modifiers { alt: true, ..Modifiers::NONE };
        assert_eq!(recorder.step(alt, &[]), None);
        assert_eq!(recorder.step(Modifiers::NONE, &[]).as_deref(), Some("alt"));
    }

    #[test]
    fn records_the_cmd_and_windows_keys() {
        let mut recorder = Recorder::default();
        let cmd = Modifiers { mac_cmd: true, ..Modifiers::NONE };
        let finished = recorder.step(cmd, &[press(Key::T, cmd)]).unwrap();
        assert_eq!(finished, format!("{META}+t"));
    }

    fn physical(key: Key, pressed: bool) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn records_the_windows_or_super_key_alone_and_in_a_combination() {
        // On Windows and Linux these only arrive as key events.
        let mut recorder = Recorder::default();
        assert_eq!(recorder.step(Modifiers::NONE, &[physical(Key::SuperLeft, true)]), None);
        assert_eq!(recorder.progress(), META);
        let finished = recorder.step(Modifiers::NONE, &[physical(Key::SuperLeft, false)]);
        assert_eq!(finished.as_deref(), Some(META));

        let mut recorder = Recorder::default();
        assert_eq!(recorder.step(Modifiers::NONE, &[physical(Key::SuperRight, true)]), None);
        let finished = recorder.step(Modifiers::NONE, &[press(Key::E, Modifiers::NONE)]).unwrap();
        assert_eq!(finished, format!("{META}+e"));
    }

    #[test]
    fn a_modifier_key_going_down_does_not_end_the_recording() {
        let mut recorder = Recorder::default();
        let shift = Modifiers { shift: true, ..Modifiers::NONE };
        for key in [Key::ShiftLeft, Key::ShiftRight, Key::ControlLeft, Key::AltRight] {
            assert_eq!(recorder.step(shift, &[physical(key, true)]), None, "{key:?}");
        }
        let finished = recorder.step(shift, &[press(Key::F13, shift)]);
        assert_eq!(finished.as_deref(), Some("shift+f13"));
    }

    #[test]
    fn records_function_keys_and_clipboard_shortcuts() {
        let mut recorder = Recorder::default();
        assert_eq!(recorder.step(Modifiers::NONE, &[press(Key::F13, Modifiers::NONE)]).as_deref(), Some("f13"));
        let mut recorder = Recorder::default();
        let finished = recorder.step(Modifiers::COMMAND, &[Event::Copy]).unwrap();
        assert!(finished.ends_with("+c"), "{finished}");
    }

    #[test]
    fn the_key_list_toggles_modifiers_and_sets_the_main_key() {
        assert_eq!(add_key("", "ctrl"), "ctrl");
        assert_eq!(add_key("ctrl", "m"), "ctrl+m");
        assert_eq!(add_key("ctrl+m", "shift"), "ctrl+shift+m");
        assert_eq!(add_key("ctrl+shift+m", "ctrl"), "shift+m");
        assert_eq!(add_key("shift+m", "f13"), "shift+f13");
        assert_eq!(add_key("control+m", "alt"), "ctrl+alt+m");
    }

    #[test]
    fn every_key_name_is_one_the_desktop_module_can_press() {
        for key in Key::ALL {
            let Some(name) = key_name(*key) else {
                continue;
            };
            // Function keys above what this system has are refused when pressed, not recorded wrongly.
            if name.starts_with('f') && name.len() > 1 && name[1..].chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            // These keys don't exist on a Mac keyboard, so they can't be recorded there.
            if cfg!(target_os = "macos") && ["insert", "printscreen", "pause", "numlock", "scrolllock"].contains(&name.as_str()) {
                continue;
            }
            assert!(pyroporch_desktop::parse_combo(&name).is_ok(), "{key:?} became {name}");
        }
    }

    #[test]
    fn the_list_only_offers_keys_that_can_be_pressed() {
        let lists: Vec<String> = MODIFIERS
            .iter()
            .chain(NAVIGATION.iter())
            .chain(EDITING.iter())
            .map(|name| (*name).to_string())
            .chain((1..=FUNCTION_KEYS).map(|n| format!("f{n}")))
            .collect();
        for name in lists {
            assert!(pyroporch_desktop::parse_combo(&name).is_ok(), "{name}");
        }
    }
}
