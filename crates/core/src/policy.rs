//! What the person has allowed, and the checks every plugin request goes through.

use pyroporch_sdk::{Host, HttpRequest, HttpResponse, LogLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use zeroize::Zeroizing;

use crate::paths::{Paths, audit, write_private};
use crate::security::{find_refs, scrub};
use crate::vault::Vault;

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_TIMEOUT_MS: u64 = 60_000;
const ALLOWED_METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

/// What one plugin has been allowed to do.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginGrant {
    pub enabled: bool,
    /// Fingerprint of the approved `.wasm` file. A different file needs approving again.
    pub sha256: String,
    /// Sites it may contact. `http://name` also allows unencrypted traffic to that site.
    pub http_hosts: Vec<String>,
    /// Vault secrets it may refer to. `*` means any, and each still only goes to its own sites.
    pub secrets: Vec<String>,
    /// May change the deck's brightness and play on its keys.
    pub deck: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Grants {
    pub plugins: BTreeMap<String, PluginGrant>,
}

impl Grants {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.grants())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_private(&paths.grants(), text.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn allow_host(paths: &Paths, plugin: &str, host: &str) -> Result<(), String> {
        let mut grants = Self::load(paths);
        let grant = grants.plugins.entry(plugin.to_string()).or_default();
        if !grant.http_hosts.iter().any(|h| h == host) {
            grant.http_hosts.push(host.to_string());
        }
        grants.save(paths)
    }

    pub fn revoke_host(paths: &Paths, plugin: &str, host: &str) -> Result<(), String> {
        let mut grants = Self::load(paths);
        if let Some(grant) = grants.plugins.get_mut(plugin) {
            grant.http_hosts.retain(|h| h != host);
        }
        grants.save(paths)
    }

    /// `Some(true)` if allowed over any transport, `Some(false)` if encrypted only.
    fn host_rule(&self, plugin: &str, authority: &str) -> Option<bool> {
        let hosts = &self.plugins.get(plugin)?.http_hosts;
        if hosts.iter().any(|h| h.strip_prefix("http://") == Some(authority)) {
            Some(true)
        } else if hosts.iter().any(|h| h == authority) {
            Some(false)
        } else {
            None
        }
    }

    /// Whether this plugin may contact that site, over https or better. This is the one rule
    /// for "has the person allowed this site", so every caller answers the same way.
    pub fn allows_host(&self, plugin: &str, authority: &str) -> bool {
        self.host_rule(plugin, authority).is_some()
    }
}

/// A request waiting for the person's answer in the app.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostRequest {
    pub plugin: String,
    pub host: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretRequest {
    pub name: String,
    pub hosts: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pending {
    pub hosts: Vec<HostRequest>,
    pub secrets: Vec<SecretRequest>,
}

impl Pending {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.pending())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_private(&paths.pending(), text.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn request_host(paths: &Paths, plugin: &str, host: &str, reason: &str) {
        let mut pending = Self::load(paths);
        if !pending.hosts.iter().any(|r| r.plugin == plugin && r.host == host) {
            pending.hosts.push(HostRequest {
                plugin: plugin.into(),
                host: host.into(),
                reason: reason.into(),
            });
            let _ = pending.save(paths);
        }
    }

    pub fn request_secret(paths: &Paths, name: &str, hosts: &[String], reason: &str) {
        let mut pending = Self::load(paths);
        if !pending.secrets.iter().any(|r| r.name == name) {
            pending.secrets.push(SecretRequest {
                name: name.into(),
                hosts: hosts.to_vec(),
                reason: reason.into(),
            });
            let _ = pending.save(paths);
        }
    }
}

/// The site part of an address, as used in grants: a name, plus the port if it isn't the default.
pub fn authority_of(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.to_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The plugin's view of the app. One is made for each run, holding what that plugin may use.
pub struct PolicyHost {
    pub plugin_id: String,
    pub paths: Paths,
    pub vault: Arc<Mutex<Vault>>,
    /// May refer to any secret; the secret's own allowed sites still apply. For built-ins.
    pub any_secret: bool,
    /// False for keys pressed by an assistant, unless the person has opted in.
    pub secrets_allowed: bool,
    /// Who pressed the key, for the activity record.
    pub actor: String,
    /// What the person has allowed, read the first time it is needed rather than for every
    /// request.
    pub grants: OnceLock<Grants>,
}

impl PolicyHost {
    /// What the person has allowed this plugin to do.
    fn grants(&self) -> &Grants {
        self.grants.get_or_init(|| Grants::load(&self.paths))
    }
}

impl PolicyHost {
    /// Replaces the placeholders in a piece of text, remembering what was put in.
    fn fill(&self, text: &str, authority: &str, used: &mut Vec<Zeroizing<String>>) -> Result<String, String> {
        let refs = find_refs(text);
        if refs.is_empty() {
            return Ok(text.to_string());
        }
        if !self.secrets_allowed {
            return Err("an assistant can't use vault secrets. Press this key in the app instead.".into());
        }
        let allowed: &[String] = self
            .grants()
            .plugins
            .get(&self.plugin_id)
            .map_or(&[], |g| g.secrets.as_slice());
        let mut vault = self.vault.lock().map_err(|_| "the vault is unavailable")?;
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for reference in refs {
            if !self.any_secret && !allowed.contains(&reference.name) {
                return Err(format!("this plugin wasn't given access to the secret {}", reference.name));
            }
            let value = vault.reveal_for(&reference.name, authority)?;
            out.push_str(&text[at..reference.range.start]);
            match &reference.basic_user {
                Some(user) => {
                    use base64::Engine as _;
                    let pair = Zeroizing::new(format!("{user}:{}", value.as_str()));
                    let encoded = base64::engine::general_purpose::STANDARD.encode(pair.as_bytes());
                    out.push_str(&encoded);
                    used.push(Zeroizing::new(encoded));
                }
                None => out.push_str(&value),
            }
            at = reference.range.end;
            used.push(value);
        }
        out.push_str(&text[at..]);
        Ok(out)
    }
}

impl Host for PolicyHost {
    fn http(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let method = request.method.to_uppercase();
        if !ALLOWED_METHODS.contains(&method.as_str()) {
            return Err(format!("unsupported method: {method}"));
        }
        // The site is decided from the address as written, before any secret is filled in, so a
        // secret can never change where a request goes.
        let (before_query, _) = request.url.split_once('?').unwrap_or((&request.url, ""));
        if before_query.contains("{{") {
            return Err("a secret can only go in the part of an address after the ?".into());
        }
        let parsed = url::Url::parse(&request.url.replace("{{", "%7B%7B").replace("}}", "%7D%7D"))
            .map_err(|_| "that isn't a valid address".to_string())?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("addresses can't contain a username or password; use the vault".into());
        }
        let authority = authority_of(&parsed).ok_or("that address has no site name")?;

        let Some(plain_allowed) = self.grants().host_rule(&self.plugin_id, &authority) else {
            Pending::request_host(&self.paths, &self.plugin_id, &authority, "A key tried to contact it.");
            return Err(format!("needs your OK to contact {authority}. Allow it in the app."));
        };
        match parsed.scheme() {
            "https" => {}
            "http" if plain_allowed || is_loopback(&parsed) => {}
            "http" => return Err(format!("{authority} must use https")),
            other => return Err(format!("unsupported kind of address: {other}")),
        }

        let mut used = Vec::new();
        let url = self.fill(&request.url, &authority, &mut used)?;
        let mut builder = ureq::http::Request::builder().method(method.as_str()).uri(url);
        for (name, value) in &request.headers {
            let value = self.fill(value, &authority, &mut used)?;
            if value.contains(['\r', '\n']) || name.contains(['\r', '\n', ':']) {
                return Err("a header contains a line break, so it was refused".into());
            }
            builder = builder.header(name.as_str(), value);
        }
        let body = match &request.body {
            Some(body) => Some(self.fill(body, &authority, &mut used)?),
            None => None,
        };

        let timeout = request.timeout_ms.unwrap_or(15_000).clamp(100, MAX_TIMEOUT_MS);
        // Redirects aren't followed: they could carry a secret to a site nobody approved.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_millis(timeout)))
            .build()
            .into();
        let secrets = Zeroizing::new(used.iter().map(|s| s.to_string()).collect::<Vec<String>>());
        let clean = |text: &str| scrub(text, &secrets);
        let response = match body {
            Some(body) => {
                let request = builder.body(body).map_err(|e| clean(&e.to_string()))?;
                agent.run(request)
            }
            None => {
                let request = builder.body(()).map_err(|e| clean(&e.to_string()))?;
                agent.run(request)
            }
        };
        let mut response = match response {
            Ok(response) => response,
            Err(e) => {
                audit(&self.paths, &self.actor, &format!("{} {method} {authority} failed", self.plugin_id));
                return Err(clean(&format!("couldn't reach {authority}: {e}")));
            }
        };
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut bytes = Vec::new();
        let read = response
            .body_mut()
            .as_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes);
        read.map_err(|e| clean(&format!("couldn't read the answer: {e}")))?;
        let truncated = bytes.len() as u64 > MAX_RESPONSE_BYTES;
        bytes.truncate(MAX_RESPONSE_BYTES as usize);
        audit(&self.paths, &self.actor, &format!("{} {method} {authority} answered {status}", self.plugin_id));
        Ok(HttpResponse {
            status,
            content_type,
            body: clean(&String::from_utf8_lossy(&bytes)),
            truncated,
        })
    }

    fn log(&self, level: LogLevel, message: &str) {
        if level != LogLevel::Info {
            audit(&self.paths, &self.plugin_id, message);
        }
    }

    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::KdfParams;
    use std::io::Write;
    use std::net::TcpListener;

    const FAST: KdfParams = KdfParams {
        memory_kib: 1024,
        passes: 1,
        lanes: 1,
    };

    /// A tiny local server. Answers one request with the given reply and returns what it received.
    fn serve_once(reply: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buf = [0u8; 4096];
            while !received.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
            }
            write!(stream, "{reply}").unwrap();
            String::from_utf8_lossy(&received).into_owned()
        });
        (port, handle)
    }

    fn setup(any_secret: bool) -> (tempfile::TempDir, PolicyHost) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        paths.ensure().unwrap();
        let mut vault = Vault::with_kdf(paths.vault(), FAST).unwrap();
        vault.create("correct horse battery").unwrap();
        let host = PolicyHost {
            plugin_id: "test.plugin".into(),
            paths,
            vault: Arc::new(Mutex::new(vault)),
            any_secret,
            secrets_allowed: true,
            actor: "test".into(),
            grants: OnceLock::new(),
        };
        (dir, host)
    }

    fn get(url: String, headers: Vec<(String, String)>) -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url,
            headers,
            ..HttpRequest::default()
        }
    }

    #[test]
    fn unapproved_site_is_refused_and_queued_for_the_person() {
        let (_dir, host) = setup(true);
        let error = host.http(&get("https://evil.example/x".into(), vec![])).unwrap_err();
        assert!(error.contains("needs your OK"), "{error}");
        let pending = Pending::load(&host.paths);
        assert_eq!(pending.hosts[0].host, "evil.example");
    }

    #[test]
    fn plain_http_to_a_remote_site_is_refused_even_if_allowed() {
        let (_dir, host) = setup(true);
        Grants::allow_host(&host.paths, "test.plugin", "api.example.com").unwrap();
        let error = host.http(&get("http://api.example.com/".into(), vec![])).unwrap_err();
        assert!(error.contains("must use https"), "{error}");
    }

    #[test]
    fn secret_is_sent_to_its_site_and_scrubbed_from_the_answer() {
        let (port, server) = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 18\r\nConnection: close\r\n\r\necho: s3cret-value",
        );
        let (_dir, host) = setup(true);
        let authority = format!("127.0.0.1:{port}");
        Grants::allow_host(&host.paths, "test.plugin", &authority).unwrap();
        host.vault.lock().unwrap().set("key", "s3cret-value", std::slice::from_ref(&authority)).unwrap();

        let response = host
            .http(&get(
                format!("http://{authority}/path"),
                vec![("Authorization".into(), "Bearer {{secret:key}}".into())],
            ))
            .unwrap();

        let received = server.join().unwrap();
        assert!(received.contains("Bearer s3cret-value"), "{received}");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "echo: •••", "the plugin must not get it back");
        let audit_log = std::fs::read_to_string(host.paths.audit()).unwrap();
        assert!(!audit_log.contains("s3cret"), "{audit_log}");
    }

    #[test]
    fn basic_credentials_are_built_by_the_host() {
        let (port, server) = serve_once("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        let (_dir, host) = setup(true);
        let authority = format!("127.0.0.1:{port}");
        Grants::allow_host(&host.paths, "test.plugin", &authority).unwrap();
        host.vault.lock().unwrap().set("pw", "open-sesame", std::slice::from_ref(&authority)).unwrap();
        host.http(&get(
            format!("http://{authority}/"),
            vec![("Authorization".into(), "Basic {{basic:ali:pw}}".into())],
        ))
        .unwrap();
        let received = server.join().unwrap();
        // base64 of "ali:open-sesame"
        assert!(received.contains("Basic YWxpOm9wZW4tc2VzYW1l"), "{received}");
    }

    #[test]
    fn a_secret_bound_elsewhere_is_never_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (_dir, host) = setup(true);
        Grants::allow_host(&host.paths, "test.plugin", &authority).unwrap();
        host.vault.lock().unwrap().set("key", "value", &["api.real.example".into()]).unwrap();
        let error = host
            .http(&get(
                format!("http://{authority}/"),
                vec![("X-Key".into(), "{{secret:key}}".into())],
            ))
            .unwrap_err();
        assert!(error.contains("isn't allowed to go to"), "{error}");
    }

    #[test]
    fn a_locked_vault_sends_nothing() {
        let (_dir, host) = setup(true);
        Grants::allow_host(&host.paths, "test.plugin", "api.example.com").unwrap();
        host.vault.lock().unwrap().set("key", "value", &["api.example.com".into()]).unwrap();
        host.vault.lock().unwrap().lock();
        let error = host
            .http(&get(
                "https://api.example.com/".into(),
                vec![("X-Key".into(), "{{secret:key}}".into())],
            ))
            .unwrap_err();
        assert!(error.contains("locked"), "{error}");
    }

    #[test]
    fn third_party_plugin_needs_its_own_access_to_each_secret() {
        let (_dir, host) = setup(false);
        Grants::allow_host(&host.paths, "test.plugin", "api.example.com").unwrap();
        host.vault.lock().unwrap().set("key", "value", &["api.example.com".into()]).unwrap();
        let error = host
            .http(&get(
                "https://api.example.com/".into(),
                vec![("X-Key".into(), "{{secret:key}}".into())],
            ))
            .unwrap_err();
        assert!(error.contains("wasn't given access"), "{error}");
    }

    #[test]
    fn a_secret_cannot_steer_the_request_or_inject_headers() {
        let (_dir, host) = setup(true);
        let error = host.http(&get("https://{{secret:k}}.example.com/".into(), vec![])).unwrap_err();
        assert!(error.contains("after the ?"), "{error}");

        Grants::allow_host(&host.paths, "test.plugin", "api.example.com").unwrap();
        let error = host
            .http(&get(
                "https://api.example.com/".into(),
                vec![("X-A".into(), "one\r\nX-Injected: two".into())],
            ))
            .unwrap_err();
        assert!(error.contains("line break"), "{error}");
    }

    #[test]
    fn an_assistant_cannot_use_secrets_by_default() {
        let (_dir, mut host) = setup(true);
        host.secrets_allowed = false;
        Grants::allow_host(&host.paths, "test.plugin", "api.example.com").unwrap();
        let error = host
            .http(&get(
                "https://api.example.com/".into(),
                vec![("X-Key".into(), "{{secret:key}}".into())],
            ))
            .unwrap_err();
        assert!(error.contains("assistant"), "{error}");
    }

    #[test]
    fn redirects_are_not_followed() {
        let (port, server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: https://evil.example/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let (_dir, host) = setup(true);
        let authority = format!("127.0.0.1:{port}");
        Grants::allow_host(&host.paths, "test.plugin", &authority).unwrap();
        let response = host.http(&get(format!("http://{authority}/"), vec![])).unwrap();
        assert_eq!(response.status, 302);
        server.join().unwrap();
    }
}
