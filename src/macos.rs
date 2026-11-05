//! Makes the app a proper Mac app.
//!
//! macOS names a permission request after the app that launched a program, and a program with no
//! bundle gets no name or icon of its own. So when started as a bare program, the app puts itself
//! in a `Pyro Porch.app` next to the program and starts that through the system, which makes it
//! its own app with its own permissions.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::path::Path;
use std::process::Command;

const BUNDLE_NAME: &str = "Pyro Porch.app";
const BUNDLE_ID: &str = "com.pyroporch.app";
const PROGRAM: &str = "pyroporch";

fn info_plist() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Pyro Porch</string>
    <key>CFBundleDisplayName</key><string>Pyro Porch</string>
    <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleExecutable</key><string>{PROGRAM}</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
"#
    )
}

/// Creates or refreshes the bundle. The stamp means this only happens after a new build.
fn prepare(bundle: &Path, exe: &Path) -> Result<(), String> {
    let contents = bundle.join("Contents");
    // Kept outside the bundle: a stray file inside would fail the signature check.
    let stamp_path = bundle.with_extension("stamp");
    let stamp = env!("PYROPORCH_BUILD");
    // The stamp sits outside the bundle, so it says nothing if the bundle has been moved away.
    let in_place = contents.join("MacOS").join(PROGRAM).is_file();
    if in_place && std::fs::read_to_string(&stamp_path).is_ok_and(|found| found == stamp) {
        return Ok(());
    }
    let (macos, resources) = (contents.join("MacOS"), contents.join("Resources"));
    let fail = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(&macos).map_err(fail)?;
    std::fs::create_dir_all(&resources).map_err(fail)?;
    std::fs::write(contents.join("Info.plist"), info_plist()).map_err(fail)?;
    std::fs::write(resources.join("AppIcon.icns"), crate::icon::icns()?).map_err(fail)?;
    // Swapped into place, so a copy that's still running keeps its own file.
    let temp = macos.join(format!(".{PROGRAM}.new"));
    std::fs::copy(exe, &temp).map_err(fail)?;
    std::fs::rename(&temp, macos.join(PROGRAM)).map_err(fail)?;
    let signed = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", "--identifier", BUNDLE_ID])
        .arg(bundle)
        .output()
        .map_err(fail)?;
    if !signed.status.success() {
        return Err(String::from_utf8_lossy(&signed.stderr).into_owned());
    }
    std::fs::write(stamp_path, stamp).map_err(fail)
}

/// Starts the bundled app and waits for it to finish, returning its result. `None` means carry
/// on here: already bundled, or the bundle couldn't be made.
pub fn relaunch_in_bundle(args: &[String]) -> Option<i32> {
    let exe = std::env::current_exe().ok()?;
    if exe.to_string_lossy().contains(".app/Contents/MacOS/") || std::env::var_os("PYROPORCH_NO_BUNDLE").is_some() {
        return None;
    }
    let bundle = exe.parent()?.join(BUNDLE_NAME);
    if let Err(error) = prepare(&bundle, &exe) {
        eprintln!("pyroporch: couldn't make the app bundle, so permissions may show the wrong name: {error}");
        return None;
    }
    let mut open = Command::new("/usr/bin/open");
    open.args(["-n", "-W"]);
    // It complains harmlessly if the new copy hands over to a running one and exits at once.
    open.stderr(std::process::Stdio::null());
    if let Some(home) = std::env::var_os("PYROPORCH_HOME") {
        let mut pair = std::ffi::OsString::from("PYROPORCH_HOME=");
        pair.push(home);
        open.arg("--env").arg(pair);
    }
    open.arg(&bundle);
    if !args.is_empty() {
        open.arg("--args").args(args);
    }
    Some(open.status().ok()?.code().unwrap_or(0))
}

fn with_app(run: impl FnOnce(&NSApplication)) {
    if let Some(main_thread) = MainThreadMarker::new() {
        run(&NSApplication::sharedApplication(main_thread));
    }
}

/// While running in the background the app lives in the menu bar, not the Dock.
pub fn set_in_dock(visible: bool) {
    with_app(|app| {
        let policy = if visible {
            NSApplicationActivationPolicy::Regular
        } else {
            NSApplicationActivationPolicy::Accessory
        };
        app.setActivationPolicy(policy);
        if visible {
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    });
}

/// True when the system has made this the frontmost app.
pub fn is_active() -> bool {
    let mut active = false;
    with_app(|app| active = app.isActive());
    active
}
