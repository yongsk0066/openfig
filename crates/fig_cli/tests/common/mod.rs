#![allow(unused_imports, dead_code)]

use std::ops::{
    Deref,
    DerefMut,
};
use std::path::Path;
use std::process::Command;

pub use assert_cmd::prelude::*;
use predicates::function::FnPredicate;
pub use predicates::prelude::*;

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

pub struct Cli {
    command: Command,
    home: tempfile::TempDir,
}

impl Cli {
    pub fn home(&self) -> &Path {
        self.home.path()
    }
}
impl Deref for Cli {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}
impl DerefMut for Cli {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

/// Keep the temporary home alive through command execution; never share user state or sockets.
pub fn cli() -> Cli {
    let home = tempfile::tempdir().expect("isolated CLI home");
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("fig_cli"));
    command
        .env("HOME", home.path())
        .env("TMPDIR", home.path())
        .env_remove("XDG_RUNTIME_DIR");
    Cli { command, home }
}

pub fn is_json() -> FnPredicate<impl Fn(&str) -> bool, str> {
    predicates::function::function(|s: &str| serde_json::from_str::<serde_json::Value>(s).is_ok())
}
