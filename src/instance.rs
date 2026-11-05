//! Keeps one copy of the app per config folder, so two can't fight over the deck.
//!
//! The first copy holds a lock on a file. Starting it again asks that copy to show its window. If
//! the running copy is from an older build, it's asked to quit so the new one can take over.

use pyroporch_core::paths::{Paths, write_private};
use std::fs::{File, OpenOptions, TryLockError};
use std::time::{Duration, Instant};

const TAKEOVER_WAIT: Duration = Duration::from_secs(8);
const RETRY_EVERY: Duration = Duration::from_millis(100);

/// Held for as long as this copy runs. The system frees the lock if the app crashes.
pub struct Lock {
    _file: File,
}

pub enum Gate {
    First(Lock),
    AlreadyRunning,
}

fn try_lock(file: &File) -> Result<bool, String> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error.to_string()),
    }
}

fn open_lock(paths: &Paths) -> Result<File, String> {
    paths.ensure().map_err(|e| e.to_string())?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.instance_lock())
        .map_err(|e| format!("couldn't open the lock file: {e}"))
}

fn build_file(paths: &Paths) -> std::path::PathBuf {
    paths.root.join("instance.build")
}

/// Takes the lock, or passes the request on to the copy already running.
pub fn acquire(paths: &Paths, build: &str) -> Result<Gate, String> {
    let file = open_lock(paths)?;
    let taken = if try_lock(&file)? {
        true
    } else {
        let running = std::fs::read_to_string(build_file(paths)).unwrap_or_default();
        if running == build {
            let _ = write_private(&paths.show_request(), b"");
            return Ok(Gate::AlreadyRunning);
        }
        let _ = write_private(&paths.quit_request(), b"");
        let started = Instant::now();
        loop {
            if try_lock(&file)? {
                break true;
            }
            if started.elapsed() >= TAKEOVER_WAIT {
                break false;
            }
            std::thread::sleep(RETRY_EVERY);
        }
    };
    if !taken {
        return Err("an older copy of Pyro Porch is running and didn't quit. Quit it from the menu bar, then try again.".into());
    }
    // Requests left from an earlier run mean nothing now.
    let _ = std::fs::remove_file(paths.show_request());
    let _ = std::fs::remove_file(paths.quit_request());
    write_private(&build_file(paths), build.as_bytes()).map_err(|e| e.to_string())?;
    Ok(Gate::First(Lock { _file: file }))
}

/// Asks a running copy to quit. Says whether there was one.
pub fn request_quit(paths: &Paths) -> Result<bool, String> {
    let file = open_lock(paths)?;
    if try_lock(&file)? {
        return Ok(false);
    }
    write_private(&paths.quit_request(), b"").map_err(|e| e.to_string())?;
    Ok(true)
}

/// Takes a request if one is waiting.
pub fn take_request(path: &std::path::Path) -> bool {
    std::fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        (dir, paths)
    }

    #[test]
    fn a_second_copy_of_the_same_build_asks_the_first_to_show() {
        let (_dir, paths) = paths();
        let first = acquire(&paths, "build-1").unwrap();
        assert!(matches!(first, Gate::First(_)));
        assert!(matches!(acquire(&paths, "build-1").unwrap(), Gate::AlreadyRunning));
        assert!(take_request(&paths.show_request()));
        assert!(!take_request(&paths.show_request()), "only once");
        drop(first);
        assert!(matches!(acquire(&paths, "build-1").unwrap(), Gate::First(_)));
    }

    #[test]
    fn a_newer_build_asks_the_old_one_to_quit_and_takes_over() {
        let (_dir, paths) = paths();
        let old = acquire(&paths, "build-1").unwrap();
        let watcher = paths.clone();
        let running_copy = std::thread::spawn(move || {
            while !take_request(&watcher.quit_request()) {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(old);
        });
        assert!(matches!(acquire(&paths, "build-2").unwrap(), Gate::First(_)));
        running_copy.join().unwrap();
    }

    #[test]
    fn quit_only_reaches_a_copy_that_is_running() {
        let (_dir, paths) = paths();
        assert!(!request_quit(&paths).unwrap());
        let _running = acquire(&paths, "build-1").unwrap();
        assert!(request_quit(&paths).unwrap());
        assert!(take_request(&paths.quit_request()));
    }
}
