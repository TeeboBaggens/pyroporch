//! Finds new versions on the releases page, and swaps one in once the person asks.
//!
//! A download replaces the program itself, so nothing in it is trusted until checked. `SHA256SUMS`
//! must carry a signature from the release key, whose private half only the release workflow can
//! use, and the file must match what `SHA256SUMS` says. Each file's name holds its version, so an
//! old release can't be passed off as a new one.

use ring::signature::{ED25519, UnparsedPublicKey};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::security::sha256_hex;

const LATEST_RELEASE: &str = "https://api.github.com/repos/TeeboBaggens/pyroporch/releases/latest";
const DOWNLOADS: &str = "https://github.com/TeeboBaggens/pyroporch/releases/download/";
pub const RELEASES_PAGE: &str = "https://github.com/TeeboBaggens/pyroporch/releases";
/// How long to wait between checks, when the person has asked for them.
pub const CHECK_EVERY_SECONDS: u64 = 6 * 60 * 60;
/// The public half of the release key. The release workflow refuses to publish with a key that
/// doesn't match this.
const SIGNING_KEY_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const SUMS_FILE: &str = "SHA256SUMS";
const SIGNATURE_FILE: &str = "SHA256SUMS.sig";
/// Far bigger than a package is, so a bad answer can't fill the disk.
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// Where a new version is unpacked, next to the program so that moving it in can't fail halfway.
const STAGING_FOLDER: &str = ".pyroporch-update";

/// Only the builds on the releases page replace themselves. One built by hand is somebody's own
/// work, and could be anywhere, such as a project's build folder.
pub fn can_install() -> bool {
    option_env!("PYROPORCH_RELEASE").is_some()
}

/// A version newer than this one, and where to get it for this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The release's own page, with what changed.
    pub page: String,
    pub package: String,
    pub package_url: String,
    pub sums_url: String,
    pub signature_url: String,
}

/// `1.2.3` as numbers. A version with anything else in it isn't one this understands.
fn version_numbers(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(numbers)
}

fn is_newer(found: &str, current: &str) -> bool {
    matches!((version_numbers(found), version_numbers(current)), (Some(found), Some(current)) if found > current)
}

/// What the package for this computer is called, if there is one.
fn package_name(version: &str) -> Option<String> {
    let (system, extension) = match std::env::consts::OS {
        "linux" => ("linux", "tar.gz"),
        "macos" => ("macos", "zip"),
        "windows" if std::env::consts::ARCH == "x86_64" => ("windows", "zip"),
        _ => return None,
    };
    Some(format!("pyroporch-{version}-{system}-{}.{extension}", std::env::consts::ARCH))
}

/// Reads GitHub's answer about the latest release. Nothing if it's no newer than this version.
fn read_release(answer: &str, current: &str) -> Result<Option<Release>, String> {
    let answer: Value = serde_json::from_str(answer).map_err(|_| "the releases page gave an answer that can't be read".to_string())?;
    let tag = answer["tag_name"].as_str().ok_or("the releases page didn't name a version")?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if !is_newer(version, current) {
        return Ok(None);
    }
    let package = package_name(version).ok_or("there's no download for this kind of computer")?;
    let url_of = |name: &str| {
        let url = answer["assets"].as_array().into_iter().flatten().find(|asset| asset["name"] == name).and_then(|asset| asset["browser_download_url"].as_str());
        match url {
            Some(url) if url.starts_with(DOWNLOADS) => Ok(url.to_string()),
            Some(url) => Err(format!("version {version} points somewhere unexpected: {url}")),
            None => Err(format!("version {version} is out, but it has no {name} file")),
        }
    };
    Ok(Some(Release {
        version: version.to_string(),
        page: format!("{RELEASES_PAGE}/tag/{tag}"),
        package_url: url_of(&package)?,
        sums_url: url_of(SUMS_FILE)?,
        signature_url: url_of(SIGNATURE_FILE)?,
        package,
    }))
}

fn fetch(url: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().https_only(true).proxy(ureq::Proxy::try_from_env()).timeout_global(Some(timeout)).build().into();
    let response = agent
        .get(url)
        .header("User-Agent", concat!("pyroporch/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/octet-stream, application/vnd.github+json")
        .call()
        .map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    response.into_body().with_config().limit(MAX_DOWNLOAD_BYTES).read_to_vec().map_err(|e| format!("couldn't finish downloading: {e}"))
}

/// Asks the releases page whether there's a newer version than this one.
pub fn check() -> Result<Option<Release>, String> {
    let answer = fetch(LATEST_RELEASE, CHECK_TIMEOUT)?;
    read_release(&String::from_utf8_lossy(&answer), env!("CARGO_PKG_VERSION"))
}

fn signing_key() -> Vec<u8> {
    (0..SIGNING_KEY_HEX.len()).step_by(2).filter_map(|at| u8::from_str_radix(&SIGNING_KEY_HEX[at..at + 2], 16).ok()).collect()
}

/// The fingerprint `SHA256SUMS` gives this file, once its signature has been checked.
fn signed_fingerprint(sums: &[u8], signature: &[u8], key: &[u8], package: &str) -> Result<String, String> {
    UnparsedPublicKey::new(&ED25519, key)
        .verify(sums, signature)
        .map_err(|_| "this download isn't signed by the release key, so it wasn't used".to_string())?;
    let sums = std::str::from_utf8(sums).map_err(|_| "the list of fingerprints can't be read".to_string())?;
    sums.lines()
        .find_map(|line| {
            let (fingerprint, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start().trim_start_matches('*') == package).then(|| fingerprint.to_lowercase())
        })
        .ok_or_else(|| format!("the signed list doesn't include {package}"))
}

/// Gets the package and checks it, leaving it in `folder` only if it's the real thing.
fn download_checked(release: &Release, folder: &Path, key: &[u8], get: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<PathBuf, String> {
    let sums = get(&release.sums_url)?;
    let signature = get(&release.signature_url)?;
    let wanted = signed_fingerprint(&sums, &signature, key, &release.package)?;
    let package = get(&release.package_url)?;
    if sha256_hex(&package) != wanted {
        return Err("the download doesn't match its signed fingerprint, so it wasn't used".into());
    }
    let path = folder.join(&release.package);
    std::fs::write(&path, package).map_err(|e| format!("couldn't save the download: {e}"))?;
    Ok(path)
}

/// Runs one of the system's own tools. By full path, so nothing on the `PATH` can stand in for it.
fn run_tool(tool: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let status = std::process::Command::new(tool).args(args).status().map_err(|e| format!("couldn't start {}: {e}", tool.display()))?;
    status.success().then_some(()).ok_or_else(|| format!("{} didn't finish its job", tool.display()))
}

/// Opens the checked package, with the same tool that keeps a Mac app's signature whole when packing it.
fn unpack(package: &Path, folder: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return run_tool(Path::new("/usr/bin/ditto"), &["-x".as_ref(), "-k".as_ref(), package.as_os_str(), folder.as_os_str()]);
    #[cfg(target_os = "windows")]
    {
        let system = std::env::var_os("SystemRoot").ok_or("couldn't find the Windows folder")?;
        let tar = Path::new(&system).join("System32").join("tar.exe");
        return run_tool(&tar, &["-xf".as_ref(), package.as_os_str(), "-C".as_ref(), folder.as_os_str()]);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let tar = [Path::new("/usr/bin/tar"), Path::new("/bin/tar")].into_iter().find(|tar| tar.is_file()).ok_or("couldn't find tar")?;
        run_tool(tar, &["-xzf".as_ref(), package.as_os_str(), "-C".as_ref(), folder.as_os_str()])
    }
}

/// Makes sure the new program runs and is the version it should be, before anything gets replaced.
fn confirm_version(program: &Path, version: &str) -> Result<(), String> {
    let output = std::process::Command::new(program).arg("--version").output().map_err(|e| format!("the new version wouldn't start: {e}"))?;
    let said = String::from_utf8_lossy(&output.stdout);
    (output.status.success() && said.trim() == format!("pyroporch {version}")).then_some(()).ok_or_else(|| format!("the new program isn't version {version}: it says {}", said.trim()))
}

/// Swaps `new` in for `current`, putting things back if that fails. The old one goes to `old`,
/// because Windows only lets a running program be moved, not deleted.
fn swap_in(new: &Path, current: &Path, old: &Path) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("couldn't replace {}: {e}", current.display());
    std::fs::rename(current, old).map_err(failed)?;
    if let Err(error) = std::fs::rename(new, current) {
        let _ = std::fs::rename(old, current);
        return Err(failed(error));
    }
    Ok(())
}

/// What gets replaced for a program at `program`: the whole app on a Mac, and the program elsewhere.
fn what_to_replace(program: &Path) -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let bundle = program.ancestors().find(|path| path.extension().is_some_and(|extension| extension == "app"));
        return bundle.map(Path::to_path_buf).ok_or_else(|| "this program isn't inside Pyro Porch.app, so it can't replace itself".to_string());
    }
    Ok(program.to_path_buf())
}

/// Where the staging folder goes for what's being replaced.
fn staging_folder(target: &Path) -> Result<PathBuf, String> {
    Ok(target.parent().ok_or("couldn't find the program's folder")?.join(STAGING_FOLDER))
}

/// The same steps as [`install`], for any place and way of getting files.
fn install_at(release: &Release, target: &Path, key: &[u8], get: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<(), String> {
    let staging = staging_folder(target)?;
    let cannot_write = |e: std::io::Error| format!("can't write next to the app, in {}: {e}. Move the app somewhere you can change, such as Applications, or update it from the releases page.", staging.display());
    // Whatever an earlier try left behind goes first, because a planted file could be picked up.
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(cannot_write)?;
    }
    std::fs::create_dir(&staging).map_err(cannot_write)?;
    let installed = (|| {
        let package = download_checked(release, &staging, key, get)?;
        unpack(&package, &staging)?;
        let folder = staging.join(release.package.split(".tar.gz").next().unwrap_or("").split(".zip").next().unwrap_or(""));
        let name = target.file_name().ok_or("couldn't tell the program's name")?;
        let new = folder.join(name);
        if !new.exists() {
            return Err(format!("the package has no {}", name.to_string_lossy()));
        }
        let program = if cfg!(target_os = "macos") { new.join("Contents/MacOS/pyroporch") } else { new.clone() };
        confirm_version(&program, &release.version)?;
        swap_in(&new, target, &staging.join("replaced"))
    })();
    // On Windows the program that's still running stays until the next start clears it away.
    let _ = std::fs::remove_dir_all(&staging);
    installed
}

/// Downloads and checks a release, then puts it in place of this program. Gives back what to start.
pub fn install(release: &Release) -> Result<PathBuf, String> {
    if !can_install() {
        return Err(format!("this copy was built by hand, so it doesn't replace itself. New versions are at {RELEASES_PAGE}"));
    }
    let program = std::env::current_exe().and_then(|path| path.canonicalize()).map_err(|e| format!("couldn't find this program: {e}"))?;
    let target = what_to_replace(&program)?;
    install_at(release, &target, &signing_key(), &|url| fetch(url, DOWNLOAD_TIMEOUT))?;
    Ok(target)
}

/// Takes away what an update couldn't, while the old version was still running.
pub fn clear_leftovers() {
    let target = std::env::current_exe().and_then(|path| path.canonicalize()).ok().and_then(|program| what_to_replace(&program).ok());
    if let Some(staging) = target.and_then(|target| staging_folder(&target).ok()) {
        let _ = std::fs::remove_dir_all(staging);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    const CURRENT: &str = "1.2.3";

    fn key_pair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap()
    }

    fn answer(tag: &str) -> String {
        let assets: Vec<Value> = [package_name(tag.trim_start_matches('v')).unwrap_or_default(), SUMS_FILE.into(), SIGNATURE_FILE.into()]
            .iter()
            .map(|name| serde_json::json!({"name": name, "browser_download_url": format!("{DOWNLOADS}{tag}/{name}")}))
            .collect();
        serde_json::json!({"tag_name": tag, "assets": assets}).to_string()
    }

    #[test]
    fn only_a_newer_plain_version_counts() {
        for found in ["1.2.4", "1.3.0", "2.0.0", "1.10.0"] {
            assert!(is_newer(found, CURRENT), "{found}");
        }
        for found in ["1.2.3", "1.2.2", "0.9.9", "1.2", "1.2.3.4", "1.2.4-rc.1", "latest", ""] {
            assert!(!is_newer(found, CURRENT), "{found}");
        }
        assert!(is_newer("1.10.0", "1.9.0"), "as numbers, not as text");
    }

    #[test]
    fn the_answer_from_github_gives_the_files_for_this_computer() {
        let release = read_release(&answer("v2.0.0"), CURRENT).unwrap().unwrap();
        assert_eq!(release.version, "2.0.0");
        assert_eq!(release.page, format!("{RELEASES_PAGE}/tag/v2.0.0"));
        assert_eq!(release.package, package_name("2.0.0").unwrap());
        assert!(release.package.contains(std::env::consts::ARCH));
        assert!(release.signature_url.ends_with("SHA256SUMS.sig"));
        assert_eq!(read_release(&answer("v1.2.3"), CURRENT).unwrap(), None);
        assert!(read_release("not json", CURRENT).is_err());
        assert!(read_release("{}", CURRENT).is_err());
    }

    #[test]
    fn a_release_with_a_missing_or_strange_file_is_refused() {
        let unsigned = answer("v2.0.0").replace(SIGNATURE_FILE, "other.txt");
        assert!(read_release(&unsigned, CURRENT).unwrap_err().contains("no SHA256SUMS.sig"));
        let elsewhere = answer("v2.0.0").replace(DOWNLOADS, "https://example.com/");
        assert!(read_release(&elsewhere, CURRENT).unwrap_err().contains("unexpected"));
    }

    fn release() -> Release {
        read_release(&answer("v2.0.0"), CURRENT).unwrap().unwrap()
    }

    /// Serves a release's files, with these for its package and its signature.
    fn files(release: &Release, package: Vec<u8>, sums: String, signature: Vec<u8>) -> impl Fn(&str) -> Result<Vec<u8>, String> {
        let release = release.clone();
        move |url: &str| {
            if url == release.package_url {
                Ok(package.clone())
            } else if url == release.sums_url {
                Ok(sums.clone().into_bytes())
            } else if url == release.signature_url {
                Ok(signature.clone())
            } else {
                Err(format!("nothing at {url}"))
            }
        }
    }

    fn signed_files(release: &Release, package: &[u8]) -> impl Fn(&str) -> Result<Vec<u8>, String> {
        let sums = format!("{}  {}\n", sha256_hex(package), release.package);
        let signature = key_pair().sign(sums.as_bytes()).as_ref().to_vec();
        files(release, package.to_vec(), sums, signature)
    }

    #[test]
    fn a_properly_signed_download_passes_and_is_saved() {
        let (dir, release, key) = (tempfile::tempdir().unwrap(), release(), key_pair());
        let path = download_checked(&release, dir.path(), key.public_key().as_ref(), &signed_files(&release, b"the package")).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"the package");
    }

    #[test]
    fn a_download_that_fails_any_check_never_reaches_the_disk() {
        let (dir, release, key) = (tempfile::tempdir().unwrap(), release(), key_pair());
        let public = key.public_key().as_ref().to_vec();
        let sums = format!("{}  {}\n", sha256_hex(b"the package"), release.package);
        let signature = key.sign(sums.as_bytes()).as_ref().to_vec();
        let try_it = |get: &dyn Fn(&str) -> Result<Vec<u8>, String>| download_checked(&release, dir.path(), &public, get).unwrap_err();

        // The package was changed, and the list wasn't.
        let error = try_it(&files(&release, b"something else".to_vec(), sums.clone(), signature.clone()));
        assert!(error.contains("doesn't match"), "{error}");

        // The package and the list were both changed, as anyone who can change the release could.
        let forged = format!("{}  {}\n", sha256_hex(b"something else"), release.package);
        let error = try_it(&files(&release, b"something else".to_vec(), forged.clone(), signature.clone()));
        assert!(error.contains("isn't signed"), "{error}");

        // Signed, but by someone else.
        let stranger = Ed25519KeyPair::from_seed_unchecked(&[9; 32]).unwrap().sign(forged.as_bytes()).as_ref().to_vec();
        let error = try_it(&files(&release, b"something else".to_vec(), forged, stranger));
        assert!(error.contains("isn't signed"), "{error}");

        // No signature at all.
        let error = try_it(&files(&release, b"the package".to_vec(), sums, Vec::new()));
        assert!(error.contains("isn't signed"), "{error}");

        // A real signed list from an older release doesn't vouch for this one.
        let old_sums = format!("{}  {}\n", sha256_hex(b"the package"), package_name("1.0.0").unwrap());
        let old_signature = key.sign(old_sums.as_bytes()).as_ref().to_vec();
        let error = try_it(&files(&release, b"the package".to_vec(), old_sums, old_signature));
        assert!(error.contains("doesn't include"), "{error}");

        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_built_in_key_is_a_whole_key() {
        assert_eq!(signing_key().len(), 32);
    }

    #[test]
    fn a_failed_swap_puts_the_old_one_back() {
        let dir = tempfile::tempdir().unwrap();
        let (current, old) = (dir.path().join("program"), dir.path().join("old"));
        std::fs::write(&current, "old version").unwrap();
        assert!(swap_in(&dir.path().join("not there"), &current, &old).is_err());
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "old version");

        let new = dir.path().join("new");
        std::fs::write(&new, "new version").unwrap();
        swap_in(&new, &current, &old).unwrap();
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "new version");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "old version");
    }

    /// A whole update on this computer: a real package, unpacked by the real tool, run and moved in.
    #[cfg(unix)]
    mod whole_update {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// Makes a package holding a stand-in program, which says what version it is.
        fn package(dir: &Path, release: &Release, says: &str) -> Vec<u8> {
            let name = release.package.split(".tar.gz").next().unwrap().split(".zip").next().unwrap();
            let folder = dir.join("build").join(name);
            let program = if cfg!(target_os = "macos") { folder.join("Pyro Porch.app/Contents/MacOS/pyroporch") } else { folder.join("pyroporch") };
            std::fs::create_dir_all(program.parent().unwrap()).unwrap();
            std::fs::write(&program, format!("#!/bin/sh\necho 'pyroporch {says}'\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let archive = dir.join(&release.package);
            let build = dir.join("build");
            let ok = if cfg!(target_os = "macos") {
                std::process::Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent"]).arg(&folder).arg(&archive).status()
            } else {
                std::process::Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(&build).arg(name).status()
            };
            assert!(ok.unwrap().success());
            std::fs::read(archive).unwrap()
        }

        /// A stand-in for what's installed now.
        fn installed(dir: &Path) -> PathBuf {
            let target = if cfg!(target_os = "macos") { dir.join("Pyro Porch.app") } else { dir.join("pyroporch") };
            let program = if cfg!(target_os = "macos") { target.join("Contents/MacOS/pyroporch") } else { target.clone() };
            std::fs::create_dir_all(program.parent().unwrap()).unwrap();
            std::fs::write(program, "the old version").unwrap();
            target
        }

        fn installed_text(target: &Path) -> String {
            let program = if cfg!(target_os = "macos") { target.join("Contents/MacOS/pyroporch") } else { target.to_path_buf() };
            std::fs::read_to_string(program).unwrap()
        }

        #[test]
        fn a_good_release_replaces_the_program_and_leaves_nothing_behind() {
            let (work, home, release, key) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), release(), key_pair());
            let bytes = package(work.path(), &release, "2.0.0");
            let target = installed(home.path());
            install_at(&release, &target, key.public_key().as_ref(), &signed_files(&release, &bytes)).unwrap();
            assert!(installed_text(&target).contains("pyroporch 2.0.0"));
            assert!(!home.path().join(STAGING_FOLDER).exists());
            assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
        }

        #[test]
        fn a_package_that_turns_out_to_be_the_wrong_version_or_unsigned_changes_nothing() {
            let (work, home, release, key) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), release(), key_pair());
            let target = installed(home.path());
            let bytes = package(work.path(), &release, "1.0.0");
            let error = install_at(&release, &target, key.public_key().as_ref(), &signed_files(&release, &bytes)).unwrap_err();
            assert!(error.contains("isn't version 2.0.0"), "{error}");
            assert_eq!(installed_text(&target), "the old version");

            let stranger = Ed25519KeyPair::from_seed_unchecked(&[9; 32]).unwrap();
            let error = install_at(&release, &target, stranger.public_key().as_ref(), &signed_files(&release, &bytes)).unwrap_err();
            assert!(error.contains("isn't signed"), "{error}");
            assert_eq!(installed_text(&target), "the old version");
            assert!(!home.path().join(STAGING_FOLDER).exists());
        }
    }
}
