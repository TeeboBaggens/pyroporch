//! Make API calls from a key.
//!
//! This is a sample plugin: the app puts it in the plugins folder, where it runs in the sandbox
//! like any other, and it's the worked example of how to extend the app. Everything it does goes
//! through the public SDK, with no special access. Read it top to bottom, then see
//! `plugins/README.md`.
//!
//! 1. `info()` describes the action. The settings form, the config checker and the MCP tools are
//!    all built from this description, so there is no UI code here.
//! 2. `run()` reads the settings and asks the host to send the request.
//! 3. Secrets are referred to with placeholders. The plugin never sees the value, and the host
//!    refuses to send it anywhere the person hasn't allowed.
//! 4. `export_plugin!` at the bottom makes this crate build into the sandboxed `.wasm` file.

use pyroporch_sdk::{
    ActionSpec, Field, FieldKind, HostHandle, HttpRequest, LogLevel, Outcome, Plugin, PluginInfo,
    Risk, RunRequest, Trigger, basic_ref, export_plugin, secret_ref,
};
use serde_json::Value;

pub const PLUGIN_ID: &str = "pyroporch.api-call";
const REQUEST: &str = "request";
/// Longest live value shown on a key.
const MAX_TITLE_CHARS: usize = 24;

#[derive(Default)]
pub struct ApiCallPlugin;

impl Plugin for ApiCallPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.into(),
            name: "API Calls".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Call a web API when a key is pressed, and show the answer on the key."
                .into(),
            actions: vec![ActionSpec {
                id: REQUEST.into(),
                name: "API Request".into(),
                description: "Send a web request, with credentials from the vault if needed."
                    .into(),
                icon: "🌐".into(),
                risk: Risk::Low,
                refreshable: true,
                hidden: false,
                previews: false,
                fields: request_fields(),
            }],
        }
    }

    fn run(&self, request: &RunRequest, host: &HostHandle) -> Result<Outcome, String> {
        let method = match request.text("method") {
            "" => "GET",
            method => method,
        };
        // A timer may only read. Repeating a POST or DELETE by itself would be a nasty surprise.
        if request.trigger == Trigger::Refresh && method != "GET" {
            return Err("only GET requests can refresh on a timer".into());
        }
        let url = request.text("url");
        if url.is_empty() {
            return Err("this key has no address to call yet".into());
        }

        let mut headers = request.pairs("headers");
        if let Some(header) = auth_header(request)? {
            headers.push(header);
        }
        let body = Some(request.text("body"))
            .filter(|body| !body.is_empty())
            .map(str::to_string);
        if body.is_some() && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
            headers.push(("Content-Type".into(), "application/json".into()));
        }

        let response = host.http(&HttpRequest {
            method: method.into(),
            url: url.into(),
            headers,
            body,
            timeout_ms: request.number("timeout_seconds").map(|s| (s * 1000.0) as u64),
        })?;

        let ok = (200..300).contains(&response.status);
        host.log(
            if ok { LogLevel::Info } else { LogLevel::Warn },
            &format!("{method} answered {}", response.status),
        );
        if !ok {
            return Err(format!("the server answered {}", response.status));
        }

        let mut outcome = Outcome::message(format!("{method} answered {}", response.status));
        let pointer = request.text("show_value");
        if !pointer.is_empty() {
            outcome.title = Some(pick_value(&response.body, pointer)?);
        }
        Ok(outcome)
    }
}

fn request_fields() -> Vec<Field> {
    vec![
        Field::new("url", "Address", FieldKind::RequestUrl)
            .help("For example https://api.example.com/v1/status")
            .required(),
        Field::new(
            "method",
            "Method",
            FieldKind::Select {
                options: ["GET", "POST", "PUT", "PATCH", "DELETE"]
                    .map(String::from)
                    .to_vec(),
            },
        )
        .default("GET"),
        Field::new(
            "auth",
            "Sign in with",
            FieldKind::Select {
                options: ["none", "bearer token", "basic", "api key header"]
                    .map(String::from)
                    .to_vec(),
            },
        )
        .default("none"),
        // A Secret field: masked in the UI, stored in the vault, only a reference in the config.
        Field::new("secret", "Key or password", FieldKind::Secret)
            .help("Kept encrypted in the vault, and only sent to the sites you allow for it."),
        Field::new("username", "Username", FieldKind::Text).visible_when("auth", "basic"),
        Field::new("key_header", "Header name", FieldKind::Text)
            .default("X-Api-Key")
            .visible_when("auth", "api key header"),
        Field::new("headers", "Extra headers", FieldKind::KeyValues),
        Field::new("body", "Body", FieldKind::MultiLine),
        Field::new("show_value", "Show on key", FieldKind::Text)
            .help("Which part of a JSON answer to show, for example /data/temperature"),
        Field::new("refresh_seconds", "Refresh every (seconds)", FieldKind::Number)
            .help("0 means only when pressed. GET requests only.")
            .default(0),
        Field::new("timeout_seconds", "Give up after (seconds)", FieldKind::Number).default(15),
    ]
}

/// Builds the sign-in header from placeholders, so no credential ever passes through the plugin.
fn auth_header(request: &RunRequest) -> Result<Option<(String, String)>, String> {
    let auth = request.text("auth");
    if matches!(auth, "" | "none") {
        return Ok(None);
    }
    let name = secret_name(request.text("secret"))
        .ok_or("choose a key or password from the vault for this sign-in")?;
    Ok(Some(match auth {
        "bearer token" => ("Authorization".into(), format!("Bearer {}", secret_ref(name))),
        "basic" => {
            let username = request.text("username");
            ("Authorization".into(), format!("Basic {}", basic_ref(username, name)))
        }
        "api key header" => {
            let header = match request.text("key_header") {
                "" => "X-Api-Key",
                header => header,
            };
            (header.into(), secret_ref(name))
        }
        other => return Err(format!("unknown sign-in type: {other}")),
    }))
}

/// `{{secret:name}}` to `name`.
fn secret_name(reference: &str) -> Option<&str> {
    pyroporch_sdk::secret_name(reference)
}

/// Takes one value out of a JSON answer using a JSON pointer such as `/data/temperature`.
fn pick_value(body: &str, pointer: &str) -> Result<String, String> {
    let json: Value = serde_json::from_str(body).map_err(|_| "the answer wasn't JSON".to_string())?;
    let value = json
        .pointer(pointer)
        .ok_or_else(|| format!("the answer has nothing at {pointer}"))?;
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Ok(text.chars().take(MAX_TITLE_CHARS).collect())
}

export_plugin!(ApiCallPlugin);

#[cfg(test)]
mod tests {
    use super::*;
    use pyroporch_sdk::{Host, HttpResponse};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeHost {
        sent: Mutex<Vec<HttpRequest>>,
        status: u16,
        body: String,
    }

    impl Host for FakeHost {
        fn http(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            self.sent.lock().unwrap().push(request.clone());
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
                ..HttpResponse::default()
            })
        }
        fn log(&self, _: LogLevel, _: &str) {}
        fn now_ms(&self) -> u64 {
            0
        }
    }

    fn request(settings: Value, trigger: Trigger) -> RunRequest {
        RunRequest {
            action: REQUEST.into(),
            settings: settings.as_object().unwrap().clone(),
            trigger,
        }
    }

    fn fake(status: u16, body: &str) -> (Arc<FakeHost>, HostHandle) {
        let fake = Arc::new(FakeHost {
            status,
            body: body.into(),
            ..FakeHost::default()
        });
        (fake.clone(), fake)
    }

    #[test]
    fn sends_a_placeholder_not_a_credential() {
        let (fake, host) = fake(200, "{}");
        let settings = serde_json::json!({
            "url": "https://api.example.com/x", "auth": "bearer token", "secret": "{{secret:token}}"
        });
        ApiCallPlugin.run(&request(settings, Trigger::Press), &host).unwrap();
        let sent = fake.sent.lock().unwrap();
        assert_eq!(sent[0].headers[0].1, "Bearer {{secret:token}}");
    }

    #[test]
    fn shows_a_value_from_the_answer() {
        let (_, host) = fake(200, r#"{"data":{"temp":21.5}}"#);
        let settings = serde_json::json!({"url": "https://a.example/x", "show_value": "/data/temp"});
        let outcome = ApiCallPlugin.run(&request(settings, Trigger::Press), &host).unwrap();
        assert_eq!(outcome.title.as_deref(), Some("21.5"));
    }

    #[test]
    fn a_timer_cannot_repeat_a_write() {
        let (fake, host) = fake(200, "");
        let settings = serde_json::json!({"url": "https://a.example/x", "method": "DELETE"});
        assert!(ApiCallPlugin.run(&request(settings, Trigger::Refresh), &host).is_err());
        assert!(fake.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn an_error_status_fails_the_key() {
        let (_, host) = fake(500, "");
        let settings = serde_json::json!({"url": "https://a.example/x"});
        assert!(ApiCallPlugin.run(&request(settings, Trigger::Press), &host).is_err());
    }

    #[test]
    fn sign_in_without_a_vault_secret_is_refused() {
        let (fake, host) = fake(200, "");
        let settings = serde_json::json!({
            "url": "https://a.example/x", "auth": "bearer token", "secret": "literal-password"
        });
        assert!(ApiCallPlugin.run(&request(settings, Trigger::Press), &host).is_err());
        assert!(fake.sent.lock().unwrap().is_empty());
    }
}
