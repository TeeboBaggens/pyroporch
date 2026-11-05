//! The Desktop actions, built as part of the app: open links and apps, press hotkeys and media keys,
//! type text, run commands.
//!
//! These act on the computer directly, so they can't run in the sandbox. That's why everything
//! here except opening a web link is marked `Risk::High`: the person must approve the key in the
//! app, and the approval ends as soon as its settings change.

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use pyroporch_sdk::{ActionSpec, Field, FieldKind, HostHandle, Outcome, Plugin, PluginInfo, Risk, RunRequest, Trigger};

pub const DESKTOP_ID: &str = "pyroporch.desktop";

#[derive(Default)]
pub struct DesktopActions;

fn action(id: &str, name: &str, description: &str, icon: &str, risk: Risk, fields: Vec<Field>) -> ActionSpec {
    ActionSpec {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        icon: icon.into(),
        risk,
        refreshable: false,
        hidden: false,
        previews: false,
        fields,
    }
}

impl Plugin for DesktopActions {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: DESKTOP_ID.into(),
            name: "Desktop".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Open things and control this computer.".into(),
            actions: vec![
                action("open_url", "Open Website", "Open a web address in your browser.", "🔗", Risk::Low, vec![
                    Field::new("url", "Address", FieldKind::Url).required(),
                ]),
                action("open_path", "Open App or File", "Open an application, file or folder.", "📂", Risk::High, vec![
                    Field::new("path", "Path", FieldKind::Text).required(),
                ]),
                action("press_key", "Press a Key", "Press one key, such as F13 or Page Down.", "🔘", Risk::High, vec![
                    Field::new("keys", "Key", FieldKind::Hotkey).required(),
                ]),
                action("hotkey", "Hotkey", "Press a key combination.", "⌨", Risk::High, vec![
                    Field::new("keys", "Keys", FieldKind::Hotkey)
                        .help("Record it, or choose keys from the list")
                        .required(),
                ]),
                action("media", "Media Key", "Play, pause, skip or change the volume.", "▶", Risk::High, vec![
                    Field::new("key", "Key", FieldKind::Select {
                        options: ["play/pause", "next", "previous", "volume up", "volume down", "mute"]
                            .map(String::from)
                            .to_vec(),
                    })
                    .default("play/pause")
                    .required(),
                ]),
                action("type_text", "Type Text", "Type a piece of text.", "✏", Risk::High, vec![
                    Field::new("text", "Text", FieldKind::MultiLine).required(),
                ]),
                action("run_command", "Run Command", "Run a program with arguments.", "⚙", Risk::High, vec![
                    Field::new("program", "Program", FieldKind::Text).required(),
                    Field::new("args", "Arguments", FieldKind::Text).help("Separated by spaces."),
                ]),
            ],
        }
    }

    fn run(&self, request: &RunRequest, _host: &HostHandle) -> Result<Outcome, String> {
        // These act on the computer, so a timer must not repeat them. The action itself
        // refuses, as the plugin contract says.
        if request.trigger == Trigger::Refresh {
            return Err("this changes your computer, so it can't run on a timer".into());
        }
        match request.action.as_str() {
            "open_url" => {
                let url = request.text("url");
                let parsed = url::Url::parse(url).map_err(|_| format!("not a web address: {url}"))?;
                // Only web links: `file:` and app-specific schemes could run things unapproved.
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err("only http and https links can be opened".into());
                }
                open::that_detached(url).map_err(|e| e.to_string())?;
                Ok(Outcome::message(format!("opened {}", parsed.host_str().unwrap_or(url))))
            }
            "open_path" => {
                let path = request.text("path");
                open::that_detached(path).map_err(|e| e.to_string())?;
                Ok(Outcome::message(format!("opened {path}")))
            }
            "hotkey" | "press_key" => {
                let keys = request.text("keys");
                press_combo(keys)?;
                Ok(Outcome::message(format!("pressed {keys}")))
            }
            "media" => {
                let name = request.text("key");
                tap(media_key(name)?)?;
                Ok(Outcome::message(format!("media key: {name}")))
            }
            "type_text" => {
                input()?.text(request.text("text")).map_err(|e| e.to_string())?;
                Ok(Outcome::message("typed text"))
            }
            "run_command" => {
                let program = request.text("program");
                // No shell in between, so nothing in the settings can be read as shell syntax.
                let mut child = std::process::Command::new(program)
                    .args(request.text("args").split_whitespace())
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("couldn't start {program}: {e}"))?;
                // Waited for elsewhere, so it doesn't stay behind as a zombie once it finishes.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(Outcome::message(format!("started {program}")))
            }
            other => Err(format!("unknown action: {other}")),
        }
    }
}

fn input() -> Result<Enigo, String> {
    Enigo::new(&Settings::default()).map_err(|e| {
        format!("can't control the keyboard here (on a Mac, allow it under Privacy > Accessibility): {e}")
    })
}

fn tap(key: Key) -> Result<(), String> {
    input()?.key(key, Direction::Click).map_err(|e| e.to_string())
}

fn media_key(name: &str) -> Result<Key, String> {
    Ok(match name {
        "play/pause" | "" => Key::MediaPlayPause,
        "next" => Key::MediaNextTrack,
        "previous" => Key::MediaPrevTrack,
        "volume up" => Key::VolumeUp,
        "volume down" => Key::VolumeDown,
        "mute" => Key::VolumeMute,
        other => return Err(format!("unknown media key: {other}")),
    })
}

/// Turns `ctrl+shift+m` into keys. Only a key name, never free text, reaches the keyboard.
pub fn parse_combo(combo: &str) -> Result<Vec<Key>, String> {
    combo
        .split('+')
        .map(|part| {
            let part = part.trim().to_lowercase();
            Ok(match part.as_str() {
                "ctrl" | "control" => Key::Control,
                "shift" => Key::Shift,
                "alt" | "option" => Key::Alt,
                "cmd" | "command" | "win" | "windows" | "super" | "meta" => Key::Meta,
                "enter" | "return" => Key::Return,
                "tab" => Key::Tab,
                "esc" | "escape" => Key::Escape,
                "space" => Key::Space,
                "backspace" => Key::Backspace,
                "delete" => Key::Delete,
                "home" => Key::Home,
                "end" => Key::End,
                "pageup" => Key::PageUp,
                "pagedown" => Key::PageDown,
                "capslock" => Key::CapsLock,
                "playpause" => Key::MediaPlayPause,
                "medianext" => Key::MediaNextTrack,
                "mediaprevious" => Key::MediaPrevTrack,
                "volumeup" => Key::VolumeUp,
                "volumedown" => Key::VolumeDown,
                "volumemute" => Key::VolumeMute,
                "up" => Key::UpArrow,
                "down" => Key::DownArrow,
                "left" => Key::LeftArrow,
                "right" => Key::RightArrow,
                "plus" => Key::Unicode('+'),
                "minus" => Key::Unicode('-'),
                name if name.starts_with('f') && name.len() > 1 && name[1..].chars().all(|c| c.is_ascii_digit()) => {
                    function_key(name)?
                }
                #[cfg(not(target_os = "macos"))]
                "insert" => Key::Insert,
                #[cfg(not(target_os = "macos"))]
                "printscreen" => Key::PrintScr,
                #[cfg(not(target_os = "macos"))]
                "pause" => Key::Pause,
                #[cfg(not(target_os = "macos"))]
                "numlock" => Key::Numlock,
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                "scrolllock" => Key::ScrollLock,
                #[cfg(target_os = "windows")]
                "scrolllock" => Key::Scroll,
                single if single.chars().count() == 1 => Key::Unicode(single.chars().next().unwrap_or(' ')),
                other => return Err(format!("unknown key: {other}")),
            })
        })
        .collect()
}

/// The function keys. How many there are depends on the system.
fn function_key(name: &str) -> Result<Key, String> {
    Ok(match name {
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "f13" => Key::F13,
        "f14" => Key::F14,
        "f15" => Key::F15,
        "f16" => Key::F16,
        "f17" => Key::F17,
        "f18" => Key::F18,
        "f19" => Key::F19,
        "f20" => Key::F20,
        #[cfg(not(target_os = "macos"))]
        "f21" => Key::F21,
        #[cfg(not(target_os = "macos"))]
        "f22" => Key::F22,
        #[cfg(not(target_os = "macos"))]
        "f23" => Key::F23,
        #[cfg(not(target_os = "macos"))]
        "f24" => Key::F24,
        other => return Err(format!("{other} isn't available on this system")),
    })
}

fn press_combo(combo: &str) -> Result<(), String> {
    let keys = parse_combo(combo)?;
    let mut enigo = input()?;
    let (last, modifiers) = keys.split_last().ok_or("no keys given")?;
    for key in modifiers {
        enigo.key(*key, Direction::Press).map_err(|e| e.to_string())?;
    }
    let pressed = enigo.key(*last, Direction::Click);
    // Always let go of the modifiers, even if the main key failed.
    for key in modifiers.iter().rev() {
        let _ = enigo.key(*key, Direction::Release);
    }
    pressed.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combos() {
        let keys = parse_combo("ctrl+shift+m").unwrap();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[2], Key::Unicode('m'));
        assert!(parse_combo("ctrl+banana").is_err());
    }

    #[test]
    fn modifiers_alone_and_extra_keys_work() {
        assert_eq!(parse_combo("ctrl").unwrap(), vec![Key::Control]);
        for name in ["cmd", "win", "windows", "super"] {
            assert_eq!(parse_combo(name).unwrap(), vec![Key::Meta], "{name}");
        }
        for name in ["f13", "f20", "pageup", "pagedown", "home", "end", "delete", "backspace", "plus", "minus"] {
            assert!(parse_combo(name).is_ok(), "{name}");
        }
        assert!(parse_combo("f99").is_err());
        assert!(parse_combo("").is_err());
    }

    #[test]
    fn everything_that_acts_on_the_computer_is_high_risk() {
        for action in DesktopActions.info().actions {
            assert_eq!(action.risk == Risk::Low, action.id == "open_url", "{}", action.id);
        }
    }

    struct NoHost;
    impl pyroporch_sdk::Host for NoHost {
        fn http(&self, _request: &pyroporch_sdk::HttpRequest) -> Result<pyroporch_sdk::HttpResponse, String> {
            Err("no network here".into())
        }
        fn log(&self, _level: pyroporch_sdk::LogLevel, _message: &str) {}
        fn now_ms(&self) -> u64 {
            0
        }
    }

    #[test]
    fn a_timer_never_repeats_these() {
        let host: pyroporch_sdk::HostHandle = std::sync::Arc::new(NoHost);
        let request = RunRequest {
            action: "type_text".into(),
            settings: pyroporch_sdk::json!({"text": "hello"}).as_object().unwrap().clone(),
            trigger: Trigger::Refresh,
        };
        let error = DesktopActions.run(&request, &host).unwrap_err();
        assert!(error.contains("timer"), "{error}");
    }
}
