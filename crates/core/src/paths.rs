//! Where Pyro Porch keeps its files, and how it writes them.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// `PYROPORCH_HOME` if set, otherwise the platform's normal config folder. Settings from
    /// before the app was renamed are moved across the first time.
    pub fn from_env() -> Self {
        if let Some(root) = std::env::var_os("PYROPORCH_HOME") {
            return Self { root: root.into() };
        }
        let Some(config) = dirs::config_dir() else {
            return Self { root: ".pyroporch".into() };
        };
        let root = config.join("pyroporch");
        if let Err(error) = crate::migrate::from_old_name(&config.join("leandeck"), &root) {
            eprintln!("pyroporch: couldn't move the old settings across: {error}");
        }
        Self { root }
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }
    pub fn profile(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(format!("{id}.toml"))
    }
    /// Unsaved work on profiles. Apart from the profiles, so none of it is taken for one.
    pub fn autosave_dir(&self) -> PathBuf {
        self.root.join("autosave")
    }
    pub fn plugins_dir(&self) -> PathBuf {
        self.root.join("plugins")
    }
    pub fn vault(&self) -> PathBuf {
        self.root.join("vault.json")
    }
    pub fn grants(&self) -> PathBuf {
        self.root.join("grants.toml")
    }
    pub fn pending(&self) -> PathBuf {
        self.root.join("pending.toml")
    }
    pub fn audit(&self) -> PathBuf {
        self.root.join("audit.log")
    }
    pub fn images_dir(&self) -> PathBuf {
        self.root.join("images")
    }
    /// A stored key image. Only the file name counts, so a config can't point outside the folder.
    pub fn image(&self, name: &str) -> Option<PathBuf> {
        let safe = !name.contains(['/', '\\']) && name.ends_with(".png") && name.len() > 4;
        safe.then(|| self.images_dir().join(name))
    }
    pub fn instance_lock(&self) -> PathBuf {
        self.root.join("instance.lock")
    }
    /// Another copy of the app asks this one to show its window by creating this file.
    pub fn show_request(&self) -> PathBuf {
        self.root.join("show.request")
    }
    /// Another copy of the app, or `pyroporch --quit`, asks this one to quit by creating this file.
    pub fn quit_request(&self) -> PathBuf {
        self.root.join("quit.request")
    }

    /// Creates the folders, readable only by this user.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [self.root.clone(), self.profiles_dir(), self.autosave_dir(), self.plugins_dir(), self.images_dir()] {
            std::fs::create_dir_all(&dir)?;
            restrict(&dir, true)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn restrict(path: &Path, is_dir: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if is_dir { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _is_dir: bool) -> std::io::Result<()> {
    // On Windows, files under the user's profile folder are private to the user by default.
    Ok(())
}

/// Writes a file so that a crash never leaves it half-written, readable only by this user.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    // Unique for each save, because two threads of this process can save the same file.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let turn = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.{}.{turn}.tmp", std::process::id()));
    let result = (|| {
        // Created private to begin with: a vault file must never be readable by anyone else,
        // not even for the moment before it is moved into place.
        let mut file = open_private(&temp)?;
        restrict(&temp, false)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Opens a file that only this user may read, from the moment it exists.
#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}

/// When a file last changed, used to notice edits made outside the app.
pub fn stamp(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Adds a line to the activity record, keeping the file small. Callers only pass non-secret text.
pub fn audit(paths: &Paths, actor: &str, event: &str) {
    const MAX_BYTES: u64 = 256 * 1024;
    let path = paths.audit();
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        // Searched as bytes, because the halfway point may fall inside a multi-byte character.
        let keep = text.len() / 2;
        let start = text.as_bytes()[keep..].iter().position(|b| *b == b'\n').map_or(text.len(), |i| keep + i + 1);
        let _ = write_private(&path, &text.as_bytes()[start..]);
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = restrict(&path, false);
        let event = event.replace(['\n', '\r'], " ");
        let _ = writeln!(file, "{seconds} {actor} {event}");
    }
}
