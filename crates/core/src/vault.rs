//! The encrypted vault for API keys, passwords and tokens.
//!
//! Why not hash them? A hash is one-way: fine for checking a password someone types in, useless
//! for a key that has to be sent to a service later. So stored values are encrypted, and the only
//! password that gets the slow one-way treatment is the vault's own passphrase:
//!
//! - Passphrase to wrapping key: Argon2id, built to be slow and memory-hungry so that guessing is
//!   expensive. (Plain SHA is fast, which is exactly what you don't want here.)
//! - Vault key: 256 random bits, stored only wrapped by the wrapping key.
//! - Each secret: XChaCha20-Poly1305 with its own random nonce.
//!
//! A secret's name and allowed sites are sealed into its encryption as associated data. Editing
//! `allowed_hosts` in the file to send a key somewhere new makes that secret fail to decrypt.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

use crate::paths::{stamp, write_private};

pub const MIN_PASSPHRASE_CHARS: usize = 12;
const FORMAT: u32 = 1;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const SALT_LEN: usize = 16;
/// Stronger than the OWASP minimum for Argon2id (19 MiB, 2 passes). Checked 2026-11-03.
const DEFAULT_KDF: KdfParams = KdfParams {
    memory_kib: 64 * 1024,
    passes: 3,
    lanes: 1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct Sealed {
    nonce: String,
    data: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredSecret {
    name: String,
    allowed_hosts: Vec<String>,
    sealed: Sealed,
}

#[derive(Clone, Serialize, Deserialize)]
struct VaultFile {
    format: u32,
    kdf: KdfParams,
    salt: String,
    wrapped_key: Sealed,
    secrets: Vec<StoredSecret>,
}

/// What may be shown about a secret: never its value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecretInfo {
    pub name: String,
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    NotCreated,
    Locked,
    Unlocked,
}

pub struct Vault {
    path: PathBuf,
    file: Option<VaultFile>,
    file_stamp: Option<std::time::SystemTime>,
    key: Option<Zeroizing<[u8; KEY_LEN]>>,
    last_used: Instant,
    /// The same by the wall clock. A computer's sleep doesn't count on `Instant` everywhere, and
    /// a vault must not come back from a long sleep still open.
    last_used_wall: std::time::SystemTime,
    kdf: KdfParams,
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| format!("no secure random numbers available: {e}"))?;
    Ok(bytes)
}

fn seal(key: &[u8; KEY_LEN], aad: &[u8], plain: &[u8]) -> Result<Sealed, String> {
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let nonce = random::<NONCE_LEN>()?;
    let data = cipher
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad })
        .map_err(|_| "encryption failed".to_string())?;
    Ok(Sealed {
        nonce: B64.encode(nonce),
        data: B64.encode(data),
    })
}

fn open(key: &[u8; KEY_LEN], aad: &[u8], sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>, ()> {
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let nonce: [u8; NONCE_LEN] = B64
        .decode(&sealed.nonce)
        .map_err(|_| ())?
        .try_into()
        .map_err(|_| ())?;
    let data = B64.decode(&sealed.data).map_err(|_| ())?;
    cipher
        .decrypt(&XNonce::from(nonce), Payload { msg: &data, aad })
        .map(Zeroizing::new)
        .map_err(|_| ())
}

fn derive(passphrase: &str, salt: &[u8], kdf: KdfParams) -> Result<Zeroizing<[u8; KEY_LEN]>, String> {
    let params = argon2::Params::new(kdf.memory_kib, kdf.passes, kdf.lanes, Some(KEY_LEN))
        .map_err(|e| format!("bad key settings: {e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut())
        .map_err(|e| format!("couldn't derive the key: {e}"))?;
    Ok(out)
}

// The labels in these two functions keep the app's old name on purpose: they're sealed into every
// stored secret, and changing them would make existing vaults unreadable.
fn key_aad(kdf: KdfParams, salt: &str) -> Vec<u8> {
    format!(
        "leandeck/vault-key/v{FORMAT}\0{}\0{}\0{}\0{salt}",
        kdf.memory_kib, kdf.passes, kdf.lanes
    )
    .into_bytes()
}

fn secret_aad(name: &str, hosts: &[String]) -> Vec<u8> {
    format!("leandeck/secret/v{FORMAT}\0{name}\0{}", hosts.join("\n")).into_bytes()
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Puts allowed sites in a fixed form: lowercase, sorted, no scheme or path.
pub fn normalize_hosts(hosts: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for host in hosts {
        let host = host.trim().to_lowercase();
        let host = host
            .strip_prefix("https://")
            .or_else(|| host.strip_prefix("http://"))
            .unwrap_or(&host);
        let host = host.trim_end_matches('/');
        // An address never counts the usual https port as part of its site.
        let host = host.strip_suffix(":443").unwrap_or(host);
        if host.is_empty() {
            continue;
        }
        let bad = |c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '@' | '*' | '{' | '}');
        if host.contains(bad) {
            return Err(format!("not a site name: {host}. Use something like api.example.com"));
        }
        out.push(host.to_string());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

impl Vault {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        Self::with_kdf(path, DEFAULT_KDF)
    }

    /// `kdf` applies when a new vault is created. An existing vault keeps its own settings.
    pub fn with_kdf(path: impl Into<PathBuf>, kdf: KdfParams) -> Result<Self, String> {
        let mut vault = Self {
            path: path.into(),
            file: None,
            file_stamp: None,
            key: None,
            last_used: Instant::now(),
            last_used_wall: std::time::SystemTime::now(),
            kdf,
        };
        vault.reload()?;
        Ok(vault)
    }

    /// Reads the file again if it changed on disk.
    pub fn reload(&mut self) -> Result<(), String> {
        let now = stamp(&self.path);
        if now == self.file_stamp && (now.is_some() == self.file.is_some()) {
            return Ok(());
        }
        self.file = match std::fs::read(&self.path) {
            Ok(bytes) => {
                let file: VaultFile = serde_json::from_slice(&bytes)
                    .map_err(|_| "the vault file is damaged".to_string())?;
                if file.format != FORMAT {
                    return Err("this vault was made by a newer version".into());
                }
                Some(file)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("couldn't read the vault: {e}")),
        };
        self.file_stamp = now;
        Ok(())
    }

    fn save(&mut self) -> Result<(), String> {
        let file = self.file.as_ref().ok_or("no vault yet")?;
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
        write_private(&self.path, &bytes).map_err(|e| format!("couldn't save the vault: {e}"))?;
        self.file_stamp = stamp(&self.path);
        Ok(())
    }

    pub fn state(&self) -> VaultState {
        match (&self.file, &self.key) {
            (None, _) => VaultState::NotCreated,
            (Some(_), None) => VaultState::Locked,
            (Some(_), Some(_)) => VaultState::Unlocked,
        }
    }

    pub fn create(&mut self, passphrase: &str) -> Result<(), String> {
        if self.file.is_some() {
            return Err("a vault already exists".into());
        }
        if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
            return Err(format!("use at least {MIN_PASSPHRASE_CHARS} characters"));
        }
        let key = Zeroizing::new(random::<KEY_LEN>()?);
        let salt = B64.encode(random::<SALT_LEN>()?);
        let wrapping = derive(passphrase, &B64.decode(&salt).map_err(|e| e.to_string())?, self.kdf)?;
        let wrapped_key = seal(&wrapping, &key_aad(self.kdf, &salt), key.as_ref())?;
        self.file = Some(VaultFile {
            format: FORMAT,
            kdf: self.kdf,
            salt,
            wrapped_key,
            secrets: Vec::new(),
        });
        self.key = Some(key);
        self.touch();
        self.save()
    }

    fn unwrap_key(file: &VaultFile, passphrase: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, String> {
        let salt = B64.decode(&file.salt).map_err(|_| "the vault file is damaged".to_string())?;
        let wrapping = derive(passphrase, &salt, file.kdf)?;
        let mut plain = open(&wrapping, &key_aad(file.kdf, &file.salt), &file.wrapped_key)
            .map_err(|_| "wrong passphrase".to_string())?;
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        if plain.len() != KEY_LEN {
            plain.zeroize();
            return Err("the vault file is damaged".into());
        }
        key.copy_from_slice(&plain);
        Ok(key)
    }

    pub fn unlock(&mut self, passphrase: &str) -> Result<(), String> {
        let file = self.file.as_ref().ok_or("no vault yet")?;
        self.key = Some(Self::unwrap_key(file, passphrase)?);
        self.touch();
        Ok(())
    }

    /// Forgets the key. Everything is unreadable again until the next unlock.
    pub fn lock(&mut self) {
        self.key = None;
    }

    pub fn change_passphrase(&mut self, old: &str, new: &str) -> Result<(), String> {
        if new.chars().count() < MIN_PASSPHRASE_CHARS {
            return Err(format!("use at least {MIN_PASSPHRASE_CHARS} characters"));
        }
        let file = self.file.as_ref().ok_or("no vault yet")?;
        let key = Self::unwrap_key(file, old)?;
        let salt = B64.encode(random::<SALT_LEN>()?);
        let wrapping = derive(new, &B64.decode(&salt).map_err(|e| e.to_string())?, self.kdf)?;
        let wrapped_key = seal(&wrapping, &key_aad(self.kdf, &salt), key.as_ref())?;
        let file = self.file.as_mut().ok_or("no vault yet")?;
        file.kdf = self.kdf;
        file.salt = salt;
        file.wrapped_key = wrapped_key;
        self.save()
    }

    fn touch(&mut self) {
        self.last_used = Instant::now();
        self.last_used_wall = std::time::SystemTime::now();
    }

    /// Locks if nothing has used the vault for this long. Zero means never.
    pub fn lock_if_idle(&mut self, after: Duration) -> bool {
        let idle = self.last_used.elapsed() >= after || self.last_used_wall.elapsed().is_ok_and(|wall| wall >= after);
        if !after.is_zero() && self.key.is_some() && idle {
            self.lock();
            return true;
        }
        false
    }

    /// Names and allowed sites. Works while locked; never includes a value.
    pub fn list(&self) -> Vec<SecretInfo> {
        self.file.as_ref().map_or_else(Vec::new, |file| {
            file.secrets
                .iter()
                .map(|s| SecretInfo {
                    name: s.name.clone(),
                    allowed_hosts: s.allowed_hosts.clone(),
                })
                .collect()
        })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.list().iter().any(|s| s.name == name)
    }

    /// Adds or replaces a secret. The allowed sites are required: a key with nowhere to go is
    /// useless, and a key that may go anywhere isn't safe.
    pub fn set(&mut self, name: &str, value: &str, hosts: &[String]) -> Result<(), String> {
        if !valid_name(name) {
            return Err("names can use letters, numbers, dots, dashes and underscores".into());
        }
        if value.is_empty() {
            return Err("the value is empty".into());
        }
        let hosts = normalize_hosts(hosts)?;
        if hosts.is_empty() {
            return Err("say which sites this may be sent to".into());
        }
        let key = self.key.as_ref().ok_or("the vault is locked")?;
        let sealed = seal(key, &secret_aad(name, &hosts), value.as_bytes())?;
        let file = self.file.as_mut().ok_or("no vault yet")?;
        file.secrets.retain(|s| s.name != name);
        file.secrets.push(StoredSecret {
            name: name.into(),
            allowed_hosts: hosts,
            sealed,
        });
        file.secrets.sort_by(|a, b| a.name.cmp(&b.name));
        self.touch();
        self.save()
    }

    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let file = self.file.as_mut().ok_or("no vault yet")?;
        file.secrets.retain(|s| s.name != name);
        self.save()
    }

    /// Gives a secret's value, but only for a site it is allowed to go to.
    pub fn reveal_for(&mut self, name: &str, authority: &str) -> Result<Zeroizing<String>, String> {
        let value = self.reveal_on_timer(name, authority)?;
        self.touch();
        Ok(value)
    }

    /// The same, for a timer. Nobody is there, so it doesn't count as using the vault.
    pub fn reveal_on_timer(&mut self, name: &str, authority: &str) -> Result<Zeroizing<String>, String> {
        self.reload()?;
        let key = self.key.as_ref().ok_or("the vault is locked")?;
        let file = self.file.as_ref().ok_or("no vault yet")?;
        let secret = file
            .secrets
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("the vault has no secret called {name}"))?;
        if !secret.allowed_hosts.iter().any(|host| host == authority) {
            return Err(format!("the secret {name} isn't allowed to go to {authority}"));
        }
        let plain = open(key, &secret_aad(name, &secret.allowed_hosts), &secret.sealed)
            .map_err(|_| format!("the secret {name} has been tampered with, so it was refused"))?;
        let text = String::from_utf8(plain.to_vec()).map_err(|error| {
            // The value wasn't text. Wipe the copy the error is holding before it goes.
            let mut bytes = error.into_bytes();
            bytes.zeroize();
            "not text".to_string()
        })?;
        Ok(Zeroizing::new(text))
    }

    /// Every secret that may go to this site, so its answers can be checked for them. This isn't
    /// using the vault, so it doesn't keep it from locking. A locked vault is an error only if
    /// the site has secrets to check for.
    pub fn values_for_site(&mut self, authority: &str) -> Result<Vec<Zeroizing<String>>, String> {
        self.reload()?;
        let names: Vec<String> = self
            .list()
            .into_iter()
            .filter(|secret| secret.allowed_hosts.iter().any(|host| host == authority))
            .map(|secret| secret.name)
            .collect();
        names.iter().map(|name| self.reveal_on_timer(name, authority)).collect()
    }

    /// Remembers the vault key in the system keychain, so this device doesn't need the passphrase.
    #[cfg(feature = "os-keychain")]
    pub fn remember_on_device(&self) -> Result<(), String> {
        let key = self.key.as_ref().ok_or("the vault is locked")?;
        keychain_entry()?
            .set_secret(key.as_ref())
            .map_err(|e| format!("the system keychain refused: {e}"))
    }

    #[cfg(feature = "os-keychain")]
    pub fn forget_on_device(&self) -> Result<(), String> {
        // The entry from before the app was renamed goes too, or unlocking would still find it.
        let old = keyring::Entry::new(OLD_KEYCHAIN_SERVICE, "vault-key").map_err(|e| format!("no system keychain: {e}"))?;
        for entry in [keychain_entry()?, old] {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(format!("the system keychain refused: {e}")),
            }
        }
        Ok(())
    }

    /// Unlocks from the system keychain. Only ever called when the person asks for it, because the
    /// system may show its own prompt.
    #[cfg(feature = "os-keychain")]
    pub fn unlock_from_device(&mut self) -> Result<(), String> {
        let mut bytes = keychain_entry()?
            .get_secret()
            .or_else(|_| keyring::Entry::new(OLD_KEYCHAIN_SERVICE, "vault-key")?.get_secret())
            .map_err(|e| format!("nothing usable in the system keychain: {e}"))?;
        if bytes.len() != KEY_LEN {
            bytes.zeroize();
            return Err("the system keychain entry is damaged".into());
        }
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        key.copy_from_slice(&bytes);
        bytes.zeroize();
        let file = self.file.as_ref().ok_or("no vault yet")?;
        // Make sure this key really is this vault's key before trusting it. A vault with no
        // secrets has nothing to check the key against, and a wrong key would seal them so
        // they could never be read again, so it isn't trusted.
        if file.secrets.is_empty() {
            return Err("this vault has no secrets yet, so the device's key can't be checked against it. Use your passphrase.".into());
        }
        if file.secrets.iter().any(|s| {
            open(&key, &secret_aad(&s.name, &s.allowed_hosts), &s.sealed).is_err()
        }) {
            return Err("the system keychain holds a key for a different vault".into());
        }
        self.key = Some(key);
        self.touch();
        Ok(())
    }
}

#[cfg(feature = "os-keychain")]
const OLD_KEYCHAIN_SERVICE: &str = "leandeck-vault";

#[cfg(feature = "os-keychain")]
fn keychain_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("pyroporch-vault", "vault-key").map_err(|e| format!("no system keychain: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        memory_kib: 1024,
        passes: 1,
        lanes: 1,
    };
    const PASS: &str = "correct horse battery";

    fn setup() -> (tempfile::TempDir, Vault, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.json");
        let mut vault = Vault::with_kdf(&path, FAST).unwrap();
        vault.create(PASS).unwrap();
        (dir, vault, path)
    }

    #[test]
    fn time_asleep_counts_towards_locking() {
        let (_dir, mut vault, _path) = setup();
        let idle = std::time::Duration::from_secs(600);
        assert!(!vault.lock_if_idle(idle));
        vault.last_used_wall = std::time::SystemTime::now() - idle * 2;
        assert!(vault.lock_if_idle(idle));
    }

    #[test]
    fn the_usual_https_port_is_not_part_of_a_site() {
        let hosts = normalize_hosts(&["https://API.example.com:443/".to_string(), "b.example:8443".to_string()]).unwrap();
        assert_eq!(hosts, vec!["api.example.com".to_string(), "b.example:8443".to_string()]);
    }

    #[test]
    fn a_timer_using_a_secret_doesnt_keep_the_vault_unlocked() {
        let (_dir, mut vault, _path) = setup();
        vault.set("token", "value", &["api.example.com".into()]).unwrap();
        let idle = std::time::Duration::from_millis(100);
        std::thread::sleep(idle * 2);
        vault.reveal_for("token", "api.example.com").unwrap();
        assert!(!vault.lock_if_idle(idle), "a person using it counts");
        std::thread::sleep(idle * 2);
        vault.reveal_on_timer("token", "api.example.com").unwrap();
        assert!(vault.lock_if_idle(idle), "a timer doesn't");
    }

    #[test]
    fn value_is_never_on_disk_in_the_clear() {
        let (_dir, mut vault, path) = setup();
        vault
            .set("token", "super-secret-value-123", &["api.example.com".into()])
            .unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("super-secret-value-123"));
        assert!(!on_disk.contains(PASS));
        assert_eq!(
            vault.reveal_for("token", "api.example.com").unwrap().as_str(),
            "super-secret-value-123"
        );
    }

    #[test]
    fn wrong_passphrase_and_lock_keep_it_closed() {
        let (_dir, mut vault, path) = setup();
        vault.set("token", "value", &["a.example".into()]).unwrap();
        vault.lock();
        assert!(vault.reveal_for("token", "a.example").is_err());
        assert_eq!(vault.list().len(), 1, "names stay visible while locked");

        let mut other = Vault::with_kdf(&path, FAST).unwrap();
        assert_eq!(other.unlock("not the passphrase").unwrap_err(), "wrong passphrase");
        other.unlock(PASS).unwrap();
        assert_eq!(other.reveal_for("token", "a.example").unwrap().as_str(), "value");
    }

    #[test]
    fn secret_only_goes_to_its_allowed_site() {
        let (_dir, mut vault, _path) = setup();
        vault.set("token", "value", &["a.example".into()]).unwrap();
        assert!(vault.reveal_for("token", "evil.example").is_err());
    }

    #[test]
    fn editing_allowed_sites_in_the_file_breaks_the_secret() {
        let (_dir, mut vault, path) = setup();
        vault.set("token", "value", &["a.example".into()]).unwrap();
        let tampered = std::fs::read_to_string(&path).unwrap().replace("a.example", "evil.example");
        std::fs::write(&path, tampered).unwrap();

        let mut reopened = Vault::with_kdf(&path, FAST).unwrap();
        reopened.unlock(PASS).unwrap();
        let error = reopened.reveal_for("token", "evil.example").unwrap_err();
        assert!(error.contains("tampered"), "{error}");
    }

    #[test]
    fn passphrase_can_change_and_old_one_stops_working() {
        let (_dir, mut vault, path) = setup();
        vault.set("token", "value", &["a.example".into()]).unwrap();
        vault.change_passphrase(PASS, "an even longer passphrase").unwrap();
        let mut reopened = Vault::with_kdf(&path, FAST).unwrap();
        assert!(reopened.unlock(PASS).is_err());
        reopened.unlock("an even longer passphrase").unwrap();
        assert_eq!(reopened.reveal_for("token", "a.example").unwrap().as_str(), "value");
    }

    #[test]
    fn refuses_weak_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = Vault::with_kdf(dir.path().join("v.json"), FAST).unwrap();
        assert!(vault.create("short").is_err());
        vault.create(PASS).unwrap();
        assert!(vault.set("bad name!", "v", &["a.example".into()]).is_err());
        assert!(vault.set("ok", "v", &[]).is_err(), "must say where it may go");
        assert!(vault.set("ok", "v", &["*".into()]).is_err(), "no wildcard sites");
    }

    #[test]
    fn locks_when_idle() {
        let (_dir, mut vault, _path) = setup();
        assert!(!vault.lock_if_idle(Duration::ZERO), "zero means never");
        std::thread::sleep(Duration::from_millis(5));
        assert!(vault.lock_if_idle(Duration::from_millis(1)));
        assert_eq!(vault.state(), VaultState::Locked);
    }
}
