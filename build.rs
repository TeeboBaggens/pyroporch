//! Stamps each build, so a newly started copy can tell a running copy is out of date.

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    for path in ["src", "crates", "plugins", "build.rs", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    println!("cargo:rustc-env=PYROPORCH_BUILD={stamp}");
}
