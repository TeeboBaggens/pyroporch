//! Looking for new versions and installing one, off the window's thread, so the app stays usable.

use super::App;
use eframe::egui;
use pyroporch_core::update::{self, CHECK_EVERY_SECONDS, Release};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{SystemTime, UNIX_EPOCH};

type CheckResult = Result<Option<Release>, String>;
type InstallResult = Result<PathBuf, String>;

enum Finished {
    Checked(CheckResult),
    Installed(InstallResult),
}

pub struct Updates {
    pub checking: bool,
    pub installing: bool,
    /// A newer version that's out, waiting for the person to say go.
    pub found: Option<Release>,
    /// What the last check or install came to.
    pub note: String,
    /// False for a copy built by hand, which only says when there's something new.
    pub can_install: bool,
    pub check: Arc<dyn Fn() -> CheckResult + Send + Sync>,
    pub install: Arc<dyn Fn(&Release) -> InstallResult + Send + Sync>,
    sender: Sender<Finished>,
    receiver: Receiver<Finished>,
}

impl Updates {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            checking: false,
            installing: false,
            found: None,
            note: String::new(),
            can_install: update::can_install(),
            check: Arc::new(update::check),
            install: Arc::new(update::install),
            sender,
            receiver,
        }
    }
}

fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Starts what was installed. On a Mac that's the app, which the system has to open.
fn start(target: &PathBuf) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/open").arg("-n").arg(target).spawn()
    } else {
        std::process::Command::new(target).spawn()
    }
}

impl App {
    /// Looks on the releases page, from the person asking or from the timer.
    pub fn check_for_updates(&mut self, ctx: &egui::Context) {
        if self.updates.checking || self.updates.installing {
            return;
        }
        self.updates.checking = true;
        self.ws.settings.updates.last_checked = now_seconds();
        self.mark_settings_dirty();
        let (check, sender, ctx) = (self.updates.check.clone(), self.updates.sender.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = sender.send(Finished::Checked(check()));
            ctx.request_repaint();
        });
    }

    /// Fetches the version that was found, checks it, and swaps it in.
    pub fn install_update(&mut self, ctx: &egui::Context) {
        let Some(found) = self.updates.found.clone() else {
            return;
        };
        if self.updates.installing || self.updates.checking {
            return;
        }
        self.updates.installing = true;
        self.updates.note = format!("Downloading and checking version {}…", found.version);
        let (install, sender, ctx) = (self.updates.install.clone(), self.updates.sender.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = sender.send(Finished::Installed(install(&found)));
            ctx.request_repaint();
        });
    }

    /// Every 6 hours, once the person has asked for that. A clock set back counts as being due.
    pub fn automatic_update_check(&mut self, ctx: &egui::Context) {
        let settings = &self.ws.settings.updates;
        let now = now_seconds();
        if settings.check_automatically && (now.saturating_sub(settings.last_checked) >= CHECK_EVERY_SECONDS || settings.last_checked > now) {
            self.check_for_updates(ctx);
        }
    }

    pub fn finish_updates(&mut self, _ctx: &egui::Context) {
        while let Ok(finished) = self.updates.receiver.try_recv() {
            match finished {
                Finished::Checked(result) => {
                    self.updates.checking = false;
                    match result {
                        Ok(Some(release)) => {
                            self.updates.note.clear();
                            self.updates.found = Some(release);
                        }
                        Ok(None) => {
                            self.updates.found = None;
                            self.updates.note = format!("You're on the latest version, {}.", env!("CARGO_PKG_VERSION"));
                        }
                        // What was found earlier is still out, so it stays on offer.
                        Err(error) => self.updates.note = format!("Couldn't check for updates: {error}"),
                    }
                }
                Finished::Installed(result) => {
                    self.updates.installing = false;
                    match result {
                        Ok(target) => match start(&target) {
                            // The new copy asks this one to go too, as it's from another build.
                            Ok(_) => self.quit_now(),
                            Err(error) => self.updates.note = format!("The update is in place, but it couldn't start: {error}. Please open the app again."),
                        },
                        Err(error) => self.updates.note = format!("The update wasn't installed: {error}"),
                    }
                }
            }
        }
    }
}
