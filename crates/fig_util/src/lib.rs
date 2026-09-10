pub mod directories;
mod open;
mod shell;
pub mod terminal;

pub mod consts;
pub mod launchd_plist;

use std::path::{
    Path,
    PathBuf,
};

pub use consts::*;
pub use open::{
    open_url,
    open_url_async,
};

/// The executable of this process's parent, skipping `toolbox-exec`.
///
/// A thin wrapper over [`fig_os_shim::process_info::get_parent_process_exe`], which is now the
/// only implementation: `fig_util` used to carry a near-duplicate `process_info` module (178
/// lines, its own `Pid`/`PidExt`) that could not be faked in tests.
pub fn get_parent_process_exe() -> Option<PathBuf> {
    fig_os_shim::process_info::get_parent_process_exe(&fig_os_shim::Context::new())
}
use rand::RngExt;
pub use shell::Shell;
pub use terminal::Terminal;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io operation error")]
    IoError(#[from] std::io::Error),
    #[error("unsupported platform")]
    UnsupportedPlatform,
    #[error("unsupported architecture")]
    UnsupportedArch,
    #[error(transparent)]
    Directory(#[from] crate::directories::DirectoryError),
    #[error("process has no parent")]
    NoParentProcess,
    #[error("the shell, `{0}`, isn't supported yet")]
    UnknownShell(String),
    #[error("missing environment variable `{0}`")]
    MissingEnv(&'static str),
    #[error(transparent)]
    StrUtf8Error(#[from] std::str::Utf8Error),
    #[error("Failed to parse shell {0} version")]
    ShellVersion(Shell),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Returns a random 64 character hex string
///
/// # Example
///
/// ```
/// use fig_util::gen_hex_string;
///
/// let hex = gen_hex_string();
/// assert_eq!(hex.len(), 64);
/// ```
pub fn gen_hex_string() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill(&mut buf);
    hex::encode(buf)
}

/// Returns the path to the original executable, not the symlink
pub fn current_exe_origin() -> Result<PathBuf, Error> {
    Ok(std::env::current_exe()?.canonicalize()?)
}

#[must_use]
fn app_bundle_path_opt() -> Option<PathBuf> {
    use consts::macos::BUNDLE_CONTENTS_MACOS_PATH;

    let current_exe = current_exe_origin().ok()?;

    // Verify we have .../Bundle.app/Contents/MacOS/binary-name
    let mut parts: PathBuf = current_exe.components().rev().skip(1).take(3).collect();
    parts = parts.iter().rev().collect();

    if parts != Path::new(APP_BUNDLE_NAME).join(BUNDLE_CONTENTS_MACOS_PATH) {
        return None;
    }

    // .../Bundle.app/Contents/MacOS/binary-name -> .../Bundle.app
    current_exe.ancestors().nth(3).map(|s| s.into())
}

#[must_use]
pub fn app_bundle_path() -> PathBuf {
    app_bundle_path_opt().unwrap_or_else(|| Path::new(consts::system_paths::APPLICATIONS_DIR).join(APP_BUNDLE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_hex_string() {
        let hex = gen_hex_string();
        assert_eq!(hex.len(), 64);
    }

    #[test]
    fn test_current_exe_origin() {
        current_exe_origin().unwrap();
    }
}
