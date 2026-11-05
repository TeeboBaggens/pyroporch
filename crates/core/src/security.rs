//! Secret placeholders, the check that stops credentials landing in config files, and approvals.

use base64::Engine as _;
use pyroporch_sdk::{ActionSpec, FieldKind};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::ops::Range;
use zeroize::Zeroizing;

use crate::config::Step;

/// A placeholder found in text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    pub range: Range<usize>,
    /// Where the name is in the text, so it can be read without copying it.
    pub name_range: Range<usize>,
    pub name: String,
    /// Set for `{{basic:username:name}}`, which becomes an HTTP Basic credential.
    pub basic_user: Option<String>,
}

/// Finds every `{{secret:name}}` and `{{basic:user:name}}` in a piece of text.
pub fn find_refs(text: &str) -> Vec<SecretRef> {
    let mut refs = Vec::new();
    let mut from = 0;
    while let Some(start) = text[from..].find("{{").map(|i| i + from) {
        let Some(end) = text[start..].find("}}").map(|i| i + start) else {
            break;
        };
        let inner = &text[start + 2..end];
        // Where in the text the name sits, so it can be handed back without copying.
        let parsed = if let Some(name) = inner.strip_prefix("secret:") {
            Some((name.to_string(), None, start + 2 + "secret:".len()))
        } else if let Some(rest) = inner.strip_prefix("basic:") {
            rest.rsplit_once(':').map(|(user, name)| {
                let at = start + 2 + "basic:".len() + (rest.len() - name.len());
                (name.to_string(), Some(user.to_string()), at)
            })
        } else {
            None
        };
        if let Some((name, basic_user, name_at)) = parsed {
            refs.push(SecretRef {
                range: start..end + 2,
                name_range: name_at..name_at + name.len(),
                name,
                basic_user,
            });
        }
        from = end + 2;
    }
    refs
}

/// True when the whole text is one placeholder.
pub fn is_reference(text: &str) -> bool {
    let refs = find_refs(text);
    refs.len() == 1 && refs[0].range == (0..text.len())
}

/// The name inside a `{{secret:name}}`, when the text is exactly one placeholder.
pub fn reference_name(text: &str) -> Option<&str> {
    let refs = find_refs(text);
    if refs.len() != 1 || refs[0].range != (0..text.len()) {
        return None;
    }
    Some(&text[refs[0].name_range.clone()])
}

fn sensitive_name(name: &str) -> bool {
    let name = name.to_lowercase().replace('-', "_");
    [
        "authorization",
        "api_key",
        "apikey",
        "x_api_key",
        "access_token",
        "auth_token",
        "token",
        "secret",
        "client_secret",
        "password",
        "passwd",
        "pwd",
        "cookie",
        "set_cookie",
        "private_key",
    ]
    .iter()
    .any(|known| name == *known || name.ends_with(&format!("_{known}")))
}

/// Recognises the published formats of common credentials, wherever they turn up.
fn looks_like_credential(text: &str) -> bool {
    let text = text.trim();
    const PREFIXES: [&str; 10] = [
        "sk-",
        "sk_live_",
        "ghp_",
        "gho_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "AIza",
    ];
    let single_word = !text.contains(char::is_whitespace);
    (single_word && text.len() >= 16 && PREFIXES.iter().any(|p| text.starts_with(p)))
        || (single_word && text.starts_with("eyJ") && text.matches('.').count() == 2)
        || text.contains("-----BEGIN")
        || ((text.starts_with("Bearer ") || text.starts_with("Basic ")) && find_refs(text).is_empty())
}

fn check_url(field: &str, text: &str, findings: &mut Vec<String>) {
    // Checked as written, because a placeholder would be escaped once parsed.
    let (before_query, query) = text.split_once('?').unwrap_or((text, ""));
    if before_query.contains("{{") {
        findings.push(format!(
            "{field}: a secret can only go in the part of an address after the ?, so it can't change which site is called"
        ));
    }
    if let Ok(url) = url::Url::parse(before_query)
        && (!url.username().is_empty() || url.password().is_some())
    {
        findings.push(format!("{field}: the address contains a username or password; use the vault"));
    }
    for pair in query.split('&') {
        if let Some((name, value)) = pair.split_once('=')
            && sensitive_name(name)
            && !value.is_empty()
            && !outside_refs(value).trim().is_empty()
        {
            findings.push(format!("{field}: the address carries \"{name}\" in plain text; use the vault"));
        }
    }
}

/// What's written around the placeholders in a piece of text.
fn outside_refs(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for reference in find_refs(text) {
        out.push_str(&text[at..reference.range.start]);
        at = reference.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The word that goes in front of a credential in an `Authorization` header, or nothing.
fn is_auth_scheme(text: &str) -> bool {
    let word = text.trim().to_lowercase();
    ["", "bearer", "basic", "token", "digest", "apikey"].contains(&word.as_str())
}

fn check_value(path: &str, name: &str, value: &Value, findings: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if text.is_empty() {
                return;
            }
            let plain = if find_refs(text).is_empty() {
                sensitive_name(name) || looks_like_credential(text)
            } else {
                // A placeholder doesn't excuse what's written next to it.
                let outside = outside_refs(text);
                (sensitive_name(name) && !is_auth_scheme(&outside))
                    || outside.split_whitespace().any(looks_like_credential)
            };
            if plain {
                findings.push(format!("{path}: looks like a credential in plain text; use the vault"));
            }
        }
        Value::Number(_) if sensitive_name(name) => {
            findings.push(format!("{path}: looks like a credential in plain text; use the vault"));
        }
        Value::Object(map) => {
            for (key, inner) in map {
                check_value(&format!("{path}.{key}"), key, inner, findings);
            }
        }
        Value::Array(items) => {
            for item in items {
                check_value(path, name, item, findings);
            }
        }
        _ => {}
    }
}

/// The checks that apply to any setting, whatever the action is: a name that gives a secret
/// away, or a value in the published format of a common credential.
pub fn general_findings(settings: &Map<String, Value>) -> Vec<String> {
    let mut findings = Vec::new();
    for (key, value) in settings {
        check_value(key, key, value, &mut findings);
    }
    findings
}

/// Everything in these settings that would save a credential in plain text. Run when config is
/// loaded, before a key runs, and on anything the MCP server is asked to save.
pub fn plaintext_findings(spec: Option<&ActionSpec>, settings: &Map<String, Value>) -> Vec<String> {
    let mut findings = Vec::new();
    for (key, value) in settings {
        let kind = spec
            .and_then(|spec| spec.fields.iter().find(|f| &f.key == key))
            .map(|f| &f.kind);
        match kind {
            Some(FieldKind::Secret) => {
                // A secret field holds a reference to a secret in the vault, not a basic
                // credential: the two are filled in differently.
                let fine = match value {
                    Value::Null => true,
                    Value::String(text) => text.is_empty() || pyroporch_sdk::secret_name(text).is_some(),
                    _ => false,
                };
                if !fine {
                    findings.push(format!("{key}: this field only accepts a secret from the vault"));
                }
            }
            Some(FieldKind::Url | FieldKind::RequestUrl) => match value.as_str() {
                Some(url) => check_url(key, url, &mut findings),
                None => check_value(key, key, value, &mut findings),
            },
            _ => check_value(key, key, value, &mut findings),
        }
    }
    findings
}

/// A fingerprint of what a key does. A person's approval is tied to this, so it stops counting as
/// soon as anything changes.
pub fn approval_hash(steps: &[Step]) -> String {
    let canonical: Vec<Value> = steps
        .iter()
        .map(|step| {
            serde_json::json!({
                "action": step.action,
                "settings": Value::Object(step.settings.clone()),
                "delay_ms": step.delay_ms,
            })
        })
        .collect();
    sha256_hex(&serde_json::to_vec(&canonical).unwrap_or_default())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Replaces any copy of a secret, plain or base64, so it can't leak through an answer or a log.
pub fn scrub(text: &str, secrets: &[String]) -> String {
    let mut copies: Vec<Zeroizing<String>> = Vec::new();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        copies.push(Zeroizing::new(secret.clone()));
        copies.push(Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(secret)));
        copies.push(Zeroizing::new(url::form_urlencoded::byte_serialize(secret.as_bytes()).collect()));
    }
    // Longest first: one secret can be the start of another, which must go whole.
    copies.sort_by_key(|copy| std::cmp::Reverse(copy.len()));
    let mut out = text.to_string();
    for copy in &copies {
        out = out.replace(copy.as_str(), "•••");
    }
    out
}

/// Makes the process harder to read secrets out of. Best effort.
pub fn harden_process() {
    #[cfg(unix)]
    {
        // No core dumps: they would write the memory, and any unlocked key, to disk.
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn spec() -> ActionSpec {
        use pyroporch_sdk::Field;
        ActionSpec {
            id: "x".into(),
            name: "x".into(),
            description: String::new(),
            icon: String::new(),
            risk: Default::default(),
            refreshable: false,
            hidden: false,
            previews: false,
            fields: vec![
                Field::new("secret", "s", FieldKind::Secret),
                Field::new("url", "u", FieldKind::RequestUrl),
                Field::new("headers", "h", FieldKind::KeyValues),
                Field::new("note", "n", FieldKind::Text),
            ],
        }
    }

    #[test]
    fn a_placeholder_or_another_type_does_not_excuse_a_credential() {
        let plain = |settings: serde_json::Value| !plaintext_findings(Some(&spec()), settings.as_object().unwrap()).is_empty();
        assert!(plain(serde_json::json!({"password": 12345678})));
        assert!(plain(serde_json::json!({"password": "hunter2{{secret:x}}"})));
        assert!(plain(serde_json::json!({"Authorization": "Bearer ghp_abcdefghijklmnopqrstuvwxyz0123456789 {{secret:x}}"})));
        assert!(!plain(serde_json::json!({"Authorization": "Bearer {{secret:x}}"})));
        assert!(!plain(serde_json::json!({"password": "{{secret:x}}"})));
    }

    #[test]
    fn overlapping_secrets_are_blanked_whole() {
        let secrets = ["abc123".to_string(), "abc123def456".to_string()];
        assert_eq!(scrub("got abc123def456.", &secrets), "got •••.");
    }

    #[test]
    fn finds_refs() {
        let refs = find_refs("a {{secret:one}} b {{basic:me:two}} {{other}} {{secret:three");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "one");
        assert_eq!(refs[1].name, "two");
        assert_eq!(refs[1].basic_user.as_deref(), Some("me"));
        assert!(is_reference("{{secret:x}}"));
        assert!(!is_reference("Bearer {{secret:x}}"));
    }

    #[test]
    fn names_a_reference_only_when_it_is_the_whole_text() {
        assert_eq!(reference_name("{{secret:one}}"), Some("one"));
        assert_eq!(reference_name("{{basic:me:two}}"), Some("two"));
        assert_eq!(reference_name("{{secret:one}} {{secret:two}}"), None);
        assert_eq!(reference_name("Bearer {{secret:one}}"), None);
        assert_eq!(reference_name("not a reference"), None);
    }

    #[test]
    fn accepts_references_and_ordinary_settings() {
        let settings = map(json!({
            "secret": "{{secret:k}}",
            "url": "https://api.example.com/v1?key={{secret:k}}&page=2",
            "headers": {"Authorization": "Bearer {{secret:k}}", "Accept": "application/json"},
            "note": "turn the lights on"
        }));
        assert_eq!(plaintext_findings(Some(&spec()), &settings), Vec::<String>::new());
    }

    #[test]
    fn refuses_plain_credentials_everywhere() {
        let cases = [
            json!({"secret": "hunter2"}),
            json!({"headers": {"Authorization": "Bearer abc123"}}),
            json!({"headers": {"X-Api-Key": "abc123"}}),
            json!({"url": "https://user:pass@api.example.com/"}),
            json!({"url": "https://api.example.com/?api_key=abc123"}),
            json!({"url": "https://{{secret:k}}.example.com/"}),
            json!({"note": "ghp_abcdefghijklmnopqrstuvwxyz"}),
            json!({"password": "hunter2"}),
        ];
        for case in cases {
            let settings = map(case.clone());
            assert!(!plaintext_findings(Some(&spec()), &settings).is_empty(), "{case}");
        }
    }

    #[test]
    fn a_secret_field_takes_a_secret_and_not_a_basic_credential() {
        let settings = map(json!({"secret": "{{basic:me:k}}"}));
        assert!(!plaintext_findings(Some(&spec()), &settings).is_empty());
        let settings = map(json!({"secret": "{{secret:k}}"}));
        assert!(plaintext_findings(Some(&spec()), &settings).is_empty());
    }

    #[test]
    fn approval_ends_when_settings_change() {
        let step = |command: &str| Step {
            action: "p/a".into(),
            settings: map(json!({"program": command})),
            delay_ms: 0,
        };
        assert_eq!(approval_hash(&[step("ls")]), approval_hash(&[step("ls")]));
        assert_ne!(approval_hash(&[step("ls")]), approval_hash(&[step("rm")]));
    }

    #[test]
    fn scrubs_plain_and_encoded_copies() {
        let out = scrub("tok3n and dG9rM24=", &["tok3n".into()]);
        assert_eq!(out, "••• and •••");
    }
}
