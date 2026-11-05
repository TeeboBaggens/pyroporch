//! The Pyro Porch plugin SDK.
//!
//! A plugin is one type that implements [`Plugin`]. It describes its actions (the UI, the config
//! checker and the MCP server all build themselves from that description), and it runs them.
//!
//! Plugins never touch the network, the disk or the vault directly. Everything goes through
//! [`Host`], and the host decides what is allowed. A plugin refers to a stored secret with a
//! placeholder such as `{{secret:my_key}}`; the host fills it in at the moment of sending, so the
//! plugin never sees the value.
//!
//! See `plugins/README.md` for the walkthrough and the full reference, and
//! `plugins/_examples/api-call` and `plugins/_examples/screensaver` for worked examples.

use serde::{Deserialize, Serialize};
pub use serde_json::{self, Map, Value, json};
use std::sync::Arc;

/// Compiles every Rust example in the plugin guide as part of `cargo test`, so they can't go stale.
#[cfg(doctest)]
#[doc = include_str!("../../../plugins/README.md")]
pub struct PluginGuide;

/// Bumped when the sandbox boundary changes in a way old plugins cannot follow.
pub const ABI_VERSION: u32 = 1;

/// What a plugin tells the app about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Reverse-domain id, for example `com.example.weather`. Must match the id in `plugin.toml`.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub actions: Vec<ActionSpec>,
}

/// One thing a key can do. Everything but the id and name can usually be left to
/// `..ActionSpec::default()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Unique inside the plugin. The full id used in config files is `plugin_id/action_id`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// One character or emoji, shown on the key by default.
    #[serde(default)]
    pub icon: String,
    /// The settings form. Secret fields are masked and kept in the vault automatically.
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub risk: Risk,
    /// The key may be re-run on a timer, with [`Trigger::Refresh`], when its `refresh_seconds`
    /// setting is 1 or more. Add a `refresh_seconds` number field so the person can set it.
    #[serde(default)]
    pub refreshable: bool,
    /// Left off the action list. Keys already using it keep working.
    #[serde(default)]
    pub hidden: bool,
    /// The plugin can draw this action through [`Plugin::paint`], so the settings form offers a
    /// preview. The plugin needs `deck = true` in its `plugin.toml` for that, the same as
    /// for playing on the keys.
    #[serde(default)]
    pub previews: bool,
}

/// How much a key must be trusted before it may run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Can only do what the sandbox and its grants allow.
    #[default]
    Low,
    /// Acts on the computer directly. Must be approved by the person in the app, and the approval
    /// is void as soon as the settings change. Only actions built into the app can use this: the
    /// sandbox treats every plugin as `Low`, because it can only do what its grants allow anyway.
    High,
}

/// One setting in an action's form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub required: bool,
    /// Only show this field when another field has this value: `(other_key, value)`.
    #[serde(default)]
    pub visible_when: Option<(String, String)>,
    /// Don't show this field when another field has this value, or starts out with it:
    /// `(other_key, value)`.
    #[serde(default)]
    pub hidden_when: Option<(String, String)>,
}

impl Field {
    pub fn new(key: &str, label: &str, kind: FieldKind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            help: String::new(),
            kind,
            default: Value::Null,
            required: false,
            visible_when: None,
            hidden_when: None,
        }
    }
    pub fn help(mut self, help: &str) -> Self {
        self.help = help.into();
        self
    }
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default = value.into();
        self
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn visible_when(mut self, key: &str, value: &str) -> Self {
        self.visible_when = Some((key.into(), value.into()));
        self
    }
    pub fn hidden_when(mut self, key: &str, value: &str) -> Self {
        self.hidden_when = Some((key.into(), value.into()));
        self
    }
}

/// What kind of input a [`Field`] is, which decides the form and what its setting holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum FieldKind {
    Text,
    MultiLine,
    Number,
    /// A check box. Read it with [`RunRequest::flag`].
    Bool,
    /// A link for the action to open, rather than one it calls.
    Url,
    /// An address the action calls through the host. Needs the person's approval per site.
    RequestUrl,
    Select { options: Vec<String> },
    /// A password, API key or token. Only ever holds a `{{secret:name}}` reference.
    Secret,
    /// A key or key combination, such as `ctrl+shift+m`. The settings form can record it.
    Hotkey,
    /// Name and value pairs, such as HTTP headers. Values may contain secret references.
    KeyValues,
    /// Rows saying how the key should look for a value. Read it with [`RunRequest::rules`].
    Rules,
}

/// How a [`Rule`] tests a value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleTest {
    #[default]
    #[serde(rename = "is")]
    Is,
    #[serde(rename = "is not")]
    IsNot,
    #[serde(rename = "contains")]
    Contains,
    #[serde(rename = "above")]
    Above,
    #[serde(rename = "below")]
    Below,
    /// Always matches, so it belongs last, to catch whatever the others didn't.
    #[serde(rename = "anything else")]
    AnythingElse,
}

impl RuleTest {
    pub const ALL: [RuleTest; 6] = [RuleTest::Is, RuleTest::IsNot, RuleTest::Contains, RuleTest::Above, RuleTest::Below, RuleTest::AnythingElse];

    /// What it's called in the settings form and the config file.
    pub fn name(self) -> &'static str {
        match self {
            RuleTest::Is => "is",
            RuleTest::IsNot => "is not",
            RuleTest::Contains => "contains",
            RuleTest::Above => "above",
            RuleTest::Below => "below",
            RuleTest::AnythingElse => "anything else",
        }
    }
}

/// One row of a [`FieldKind::Rules`] setting: when a value passes the test, the key takes this look.
/// An empty `icon`, `icon_color` or `color` leaves that part of the key as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub when: RuleTest,
    pub value: String,
    pub icon: String,
    pub icon_color: String,
    pub color: String,
}

impl Rule {
    /// Text is compared without regard to capitals. `above` and `below` compare numbers, so they
    /// don't match anything that isn't one.
    pub fn matches(&self, value: &str) -> bool {
        let (found, wanted) = (value.trim().to_lowercase(), self.value.trim().to_lowercase());
        let numbers = || Some((found.parse::<f64>().ok()?, wanted.parse::<f64>().ok()?));
        match self.when {
            RuleTest::Is => found == wanted,
            RuleTest::IsNot => found != wanted,
            RuleTest::Contains => found.contains(&wanted),
            RuleTest::Above => numbers().is_some_and(|(found, wanted)| found > wanted),
            RuleTest::Below => numbers().is_some_and(|(found, wanted)| found < wanted),
            RuleTest::AnythingElse => true,
        }
    }
}

/// Why an action is being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The person pressed the key, or an assistant did.
    Press,
    /// A timer re-ran the key. An action that changes something should refuse to run from this.
    Refresh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    pub action: String,
    pub settings: Map<String, Value>,
    pub trigger: Trigger,
}

impl RunRequest {
    /// A text setting, or an empty string when it is missing.
    pub fn text(&self, key: &str) -> &str {
        self.settings.get(key).and_then(Value::as_str).unwrap_or("")
    }
    pub fn number(&self, key: &str) -> Option<f64> {
        self.settings.get(key).and_then(Value::as_f64)
    }
    /// A check box setting, or `false` when it is missing.
    pub fn flag(&self, key: &str) -> bool {
        self.settings.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
    /// The name and value pairs of a key-values setting.
    pub fn pairs(&self, key: &str) -> Vec<(String, String)> {
        self.settings
            .get(key)
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// The rows of a rules setting, in order, or none when it is missing. Fails on one that can't
    /// be read, as a key quietly ignoring its rules would look as if all was well.
    pub fn rules(&self, key: &str) -> Result<Vec<Rule>, String> {
        match self.settings.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(rules) => serde_json::from_value(rules.clone()).map_err(|e| format!("can't read the rules in {key}: {e}")),
        }
    }
}

/// What came of running an action.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Outcome {
    /// Short line shown in the activity list.
    #[serde(default)]
    pub message: String,
    /// New text to show on the key, for live values.
    #[serde(default)]
    pub title: Option<String>,
    /// A character or emoji to show on the key instead of its own icon, until the next run. Not a
    /// file: the app ignores anything that looks like a path.
    #[serde(default)]
    pub icon: Option<String>,
    /// A colour like `#FFFFFF` for the key's icon, until the next run.
    #[serde(default)]
    pub icon_color: Option<String>,
    /// A colour like `#2E8B57` to show on the key instead of its own, until the next run.
    #[serde(default)]
    pub color: Option<String>,
    /// Move the deck somewhere else.
    #[serde(default)]
    pub navigate: Option<Navigate>,
    /// Change the deck itself. Ignored unless the plugin's `plugin.toml` asked for `deck = true`
    /// and the person approved that.
    #[serde(default)]
    pub deck: Option<DeckControl>,
}

impl Outcome {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }
}

/// Where a key sends the deck. `Page` takes a page's name and `Profile` a profile's id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "to", content = "value")]
pub enum Navigate {
    NextPage,
    PreviousPage,
    Page(String),
    Profile(String),
}

/// A change to the physical deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do", content = "value")]
pub enum DeckControl {
    Sleep,
    SetBrightness(u8),
    Brighter(u8),
    Dimmer(u8),
    /// Turns a screen saver on, or off if this key turned it on. The plugin draws it through
    /// [`Plugin::paint`].
    ScreenSaver(SaverTiming),
}

/// When a screen saver plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaverTiming {
    pub start_now: bool,
    /// How many minutes with no key presses before it starts. Zero means never by itself.
    pub wait_minutes: u32,
}

/// A request to draw a frame of the whole deck. See [`Plugin::paint`] for the answer's layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaintRequest {
    /// The settings of the action that started it.
    pub settings: Map<String, Value>,
    pub rows: u32,
    pub cols: u32,
    /// How many squares to draw across and down each key.
    pub blocks_per_key: u32,
    /// How long it's been playing.
    pub millis: u64,
    /// True for the first frame, so a plugin can start fresh.
    pub restart: bool,
}

/// Dimmer keys stop here, so they can't leave the deck looking switched off. Sleep is for that.
pub const LOWEST_DIM: u8 = 5;

impl DeckControl {
    /// The brightness this leaves the deck at, if it changes it.
    pub fn brightness_after(self, current: u8) -> Option<u8> {
        match self {
            DeckControl::Sleep | DeckControl::ScreenSaver(_) => None,
            DeckControl::SetBrightness(value) => Some(value.min(100)),
            DeckControl::Brighter(step) => Some(current.saturating_add(step).min(100)),
            DeckControl::Dimmer(step) => Some(current.saturating_sub(step).max(LOWEST_DIM)),
        }
    }
}

/// A web request for the host to send. Secret placeholders may go in the headers, the body and the
/// part of the address after the `?`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HttpRequest {
    /// `GET`, `POST`, `PUT`, `PATCH`, `DELETE` or `HEAD`.
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    /// Defaults to 15 seconds, and is kept between 100 milliseconds and 60 seconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub content_type: Option<String>,
    /// As text, with any secret that was sent in the request blanked out.
    pub body: String,
    /// The answer was over 1 MiB, so `body` holds only the first 1 MiB.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// What the app offers a plugin. Every call is checked against what the person has approved.
pub trait Host: Send + Sync {
    /// Send a request. Fails if the plugin has not been allowed to reach that host, or if a secret
    /// it references is locked or is not allowed to go to that host.
    ///
    /// An error status from the site is a normal answer, not a failure, and redirects aren't
    /// followed, so check [`HttpResponse::status`].
    fn http(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
    /// Warnings and errors go to the app's activity record. `Info` isn't kept.
    fn log(&self, level: LogLevel, message: &str);
    /// The time, in milliseconds since 1970.
    fn now_ms(&self) -> u64;
}

pub type HostHandle = Arc<dyn Host>;

/// The one trait a plugin implements.
pub trait Plugin: Send + Sync {
    /// Describes the plugin. Called once, when it's loaded, with no way to reach the app.
    fn info(&self) -> PluginInfo;
    /// Runs one action. An `Err` shows on the key as the reason it failed.
    fn run(&self, request: &RunRequest, host: &HostHandle) -> Result<Outcome, String>;

    /// Draws a frame, for a plugin that offers an animation. The answer is one byte for how bright
    /// the deck should be (a percent of the person's setting), then three bytes of colour for each
    /// square, row by row across the whole deck.
    fn paint(&self, _request: &PaintRequest) -> Result<Vec<u8>, String> {
        Err("this plugin doesn't draw".into())
    }
}

/// The placeholder for a stored secret, for building headers and bodies in plugin code.
pub fn secret_ref(name: &str) -> String {
    format!("{{{{secret:{name}}}}}")
}

/// The name inside a `{{secret:name}}` reference, if the text is exactly one reference.
pub fn secret_name(reference: &str) -> Option<&str> {
    let inner = reference.strip_prefix("{{secret:")?.strip_suffix("}}")?;
    // A reference is the whole of the text, not a part of it.
    if inner.is_empty() || inner.contains("{{") || inner.contains("}}") {
        return None;
    }
    Some(inner)
}

/// The placeholder for an HTTP Basic credential made from a username and a stored secret.
pub fn basic_ref(username: &str, name: &str) -> String {
    format!("{{{{basic:{username}:{name}}}}}")
}

/// A call from a sandboxed plugin to the host, as sent across the sandbox boundary.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum HostCall {
    Http { request: HttpRequest },
    Log { level: LogLevel, message: String },
    NowMs,
}

/// Plumbing used by [`export_plugin!`] inside a `.wasm` plugin. Plugin authors don't call this.
#[cfg(target_arch = "wasm32")]
pub mod guest {
    use super::*;

    #[link(wasm_import_module = "pyroporch")]
    unsafe extern "C" {
        fn host_call(ptr: *const u8, len: usize) -> u64;
    }

    /// Hands bytes to the host as a `(pointer << 32) | length` pair; the host frees them.
    pub fn leak(bytes: Vec<u8>) -> u64 {
        let bytes = bytes.into_boxed_slice();
        let len = bytes.len() as u64;
        let ptr = Box::into_raw(bytes) as *mut u8 as u32 as u64;
        (ptr << 32) | len
    }

    pub struct GuestHost;

    fn call(request: &HostCall) -> Result<Value, String> {
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let packed = unsafe { host_call(bytes.as_ptr(), bytes.len()) };
        let (ptr, len) = ((packed >> 32) as u32 as *mut u8, packed as u32 as usize);
        // The host put its reply in memory it got from `ld_alloc`, so this side owns it now.
        let reply = unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len)) };
        serde_json::from_slice::<Result<Value, String>>(&reply).map_err(|e| e.to_string())?
    }

    impl Host for GuestHost {
        fn http(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            let reply = call(&HostCall::Http {
                request: request.clone(),
            })?;
            serde_json::from_value(reply).map_err(|e| e.to_string())
        }
        fn log(&self, level: LogLevel, message: &str) {
            let _ = call(&HostCall::Log {
                level,
                message: message.into(),
            });
        }
        fn now_ms(&self) -> u64 {
            call(&HostCall::NowMs)
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        }
    }
}

/// Makes a plugin loadable as a sandboxed `.wasm` file. It does nothing in a native build, so the
/// same crate can be tested on your own computer and shipped as a plugin.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:expr) => {
        #[cfg(target_arch = "wasm32")]
        mod __pyroporch_export {
            use super::*;
            use $crate::guest::{GuestHost, leak};
            use $crate::{HostHandle, Plugin, RunRequest};

            std::thread_local! {
                static PLUGIN: Box<dyn Plugin> = Box::new($plugin);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn ld_abi() -> u32 {
                $crate::ABI_VERSION
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn ld_alloc(len: usize) -> *mut u8 {
                Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn ld_free(ptr: *mut u8, len: usize) {
                drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len)) });
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn ld_info() -> u64 {
                PLUGIN.with(|p| leak($crate::serde_json::to_vec(&p.info()).unwrap_or_default()))
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn ld_paint(ptr: *mut u8, len: usize) -> u64 {
                let bytes = unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len)) };
                let painted = $crate::serde_json::from_slice::<$crate::PaintRequest>(&bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|request| PLUGIN.with(|p| p.paint(&request)));
                leak(painted.unwrap_or_default())
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn ld_run(ptr: *mut u8, len: usize) -> u64 {
                let bytes = unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len)) };
                let result = $crate::serde_json::from_slice::<RunRequest>(&bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|request| {
                        let host: HostHandle = std::sync::Arc::new(GuestHost);
                        PLUGIN.with(|p| p.run(&request, &host))
                    });
                leak($crate::serde_json::to_vec(&result).unwrap_or_default())
            }
        }
    };
}
