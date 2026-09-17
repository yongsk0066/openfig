use std::convert::TryInto;
use std::path::PathBuf;
use std::sync::OnceLock;

use camino::Utf8PathBuf;
use thiserror::Error;
use time::OffsetDateTime;

use crate::{
    BACKUP_DIR_NAME,
    DATA_DIR_NAME,
    RUNTIME_DIR_NAME,
};

macro_rules! utf8_dir {
    ($name:ident, $($arg:ident: $type:ty),*) => {
        paste::paste! {
            pub fn [<$name _utf8>]($($arg: $type),*) -> Result<Utf8PathBuf> {
                Ok($name($($arg),*)?.try_into()?)
            }
        }
    };
    ($name:ident) => {
        utf8_dir!($name,);
    };
}

#[derive(Debug, Error)]
pub enum DirectoryError {
    #[error("home directory not found")]
    NoHomeDirectory,
    #[error("runtime directory not found: neither XDG_RUNTIME_DIR nor TMPDIR were found")]
    NoRuntimeDirectory,
    #[error("non absolute path: {0:?}")]
    NonAbsolutePath(PathBuf),
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    TimeFormat(#[from] time::error::Format),
    #[error(transparent)]
    Utf8FromPath(#[from] camino::FromPathError),
    #[error(transparent)]
    Utf8FromPathBuf(#[from] camino::FromPathBufError),
    #[error(transparent)]
    FromVecWithNul(#[from] std::ffi::FromVecWithNulError),
    #[error(transparent)]
    IntoString(#[from] std::ffi::IntoStringError),
}

type Result<T, E = DirectoryError> = std::result::Result<T, E>;

/// The directory of the users home
///
/// - MacOS: /Users/Alice
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(DirectoryError::NoHomeDirectory)
}

/// The directory of the users `$HOME/.local/bin` directory
///
/// MacOS path: `$HOME/.local/bin``
pub fn home_local_bin() -> Result<PathBuf> {
    let mut path = home_dir()?;
    path.push(".local/bin");
    Ok(path)
}

/// The app data directory
///
/// - MacOS: `$HOME/Library/Application Support/{data_dir}`
pub fn fig_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or(DirectoryError::NoHomeDirectory)?
        .join(DATA_DIR_NAME))
}

/// Get the macos tempdir from the `confstr` function
///
/// See: <https://man7.org/linux/man-pages/man3/confstr.3.html>
fn macos_tempdir() -> Result<PathBuf> {
    // SAFETY:
    // Operation: the `libc::confstr` FFI call, sizing probe.
    // Contract (POSIX `confstr(name, buf, len)`): when `len` is 0, `buf` may be NULL and the
    // function only returns the buffer size required, including the terminating NUL; it writes
    // nothing. It returns 0 if `name` is invalid.
    // Evidence: `len` is the literal 0, which is exactly the case in which a NULL `buf` is
    // permitted; `_CS_DARWIN_USER_TEMP_DIR` is a Darwin-defined `name`.
    // Postcondition: `len` is the required byte count including the NUL, or 0 on failure.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null::<i8>().cast_mut(), 0) };
    let mut buf: Vec<u8> = vec![0; len];
    // SAFETY:
    // Operation: the `libc::confstr` FFI call, fetch.
    // Contract (POSIX): `buf` must be writable for `len` bytes; the function writes at most
    // `len` bytes including a NUL, truncating if the value does not fit.
    // Evidence: `buf` is a `Vec<u8>` of exactly `len` zeroed bytes, so `buf.as_mut_ptr()` is
    // non-null and aligned (alignment 1) and writable for `buf.len()` bytes — the same value
    // passed as the size. `Vec` is not reallocated between the two lines.
    // Note on the `len == 0` case: `vec![0; 0]` has a dangling-but-aligned non-null pointer and
    // `buf.len()` is 0, which POSIX permits (write nothing); the subsequent
    // `CString::from_vec_with_nul(vec![])` then returns `Err`, so the failure surfaces as an
    // error rather than UB.
    // Robustness note: this second return value is discarded, so a truncated or failed fetch is
    // only detected indirectly by `from_vec_with_nul`.
    unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    let c_string = std::ffi::CString::from_vec_with_nul(buf)?;
    let str = c_string.into_string()?;
    Ok(PathBuf::from(str))
}

/// Runtime dir is used for runtime data that should not be persisted for a long time, e.g. socket
/// files and logs
///
/// On macOS this falls back to TMPDIR, which is secure on macOS
fn runtime_dir() -> Result<PathBuf> {
    // Cached: every socket path and log path in the process goes through here, and the
    // `confstr(_CS_DARWIN_USER_TEMP_DIR)` fallback is two syscalls. Nothing here can change during
    // the life of the process.
    static RUNTIME_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    RUNTIME_DIR
        .get_or_init(|| {
            dirs::runtime_dir()
                .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
                // `.or(Some(macos_tempdir()?))` was eager: it ran `confstr` even when `$TMPDIR`
                // was set, and turned a fallback failure into a hard error for callers that had a
                // perfectly good directory already.
                .or_else(|| macos_tempdir().ok())
        })
        .clone()
        .ok_or(DirectoryError::NoRuntimeDirectory)
}

/// The sockets directory of the local installation
///
/// - MacOS: `$TMPDIR/{runtime_dir}`
fn sockets_dir() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(RUNTIME_DIR_NAME))
}

/// The directory on the host machine where socket files are stored
///
/// - MacOS: `$TMPDIR/{runtime_dir}`
fn host_sockets_dir() -> Result<PathBuf> {
    sockets_dir()
}

/// The path to all of the themes
pub fn themes_dir() -> Result<PathBuf> {
    Ok(resources_path()?.join("themes"))
}

/// The autocomplete directory
fn autocomplete_dir() -> Result<PathBuf> {
    Ok(fig_data_dir()?.join("autocomplete"))
}

/// The autocomplete specs directory
pub fn autocomplete_specs_dir() -> Result<PathBuf> {
    Ok(autocomplete_dir()?.join("specs"))
}

/// The directory holding all of the logs
///
/// - MacOS: `$TMPDIR/{data_dir_name}log`
pub fn logs_dir() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(format!("{DATA_DIR_NAME}log")))
}

/// The directory where fig places all data-sensitive backups
///
/// - MacOS: `$HOME/{backup_dir}`
fn backups_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(BACKUP_DIR_NAME))
}

/// The directory for time based data-sensitive backups
///
/// NOTE: This changes every second and cannot be cached
pub fn utc_backup_dir() -> Result<PathBuf> {
    let now = OffsetDateTime::now_utc().format(time::macros::format_description!(
        "[year]-[month]-[day]_[hour]-[minute]-[second]"
    ))?;

    Ok(backups_dir()?.join(now))
}

/// The desktop app socket path
///
/// - MacOS: `$TMPDIR/{runtime_dir}/desktop.sock`
pub fn desktop_socket_path() -> Result<PathBuf> {
    Ok(host_sockets_dir()?.join("desktop.sock"))
}

/// The socket the zsh hook (`fig.zsh`) connects to, speaking the line protocol of
/// `fig_shell_ipc::hook`.
///
/// - MacOS: `$TMPDIR/{runtime_dir}/hook.sock`
pub fn hook_socket_path() -> Result<PathBuf> {
    Ok(host_sockets_dir()?.join("hook.sock"))
}

/// The path to the resources directory
///
/// - MacOS: "/Applications/{app_name}.app/Contents/Resources"
pub fn resources_path() -> Result<PathBuf> {
    Ok(crate::app_bundle_path().join(crate::macos::BUNDLE_CONTENTS_RESOURCE_PATH))
}

/// The path to the fig settings file
///
/// - MacOS: `$HOME/Library/Application Support/{data_dir}/settings.json`
pub fn settings_path() -> Result<PathBuf> {
    Ok(fig_data_dir()?.join("settings.json"))
}

utf8_dir!(home_dir);
utf8_dir!(fig_data_dir);
utf8_dir!(sockets_dir);
utf8_dir!(backups_dir);
utf8_dir!(logs_dir);

#[cfg(test)]
mod tests {
    use insta;

    use super::*;

    #[test]
    fn all_paths() {
        assert!(home_dir().is_ok());
        assert!(home_local_bin().is_ok());
        assert!(fig_data_dir().is_ok());
        assert!(sockets_dir().is_ok());
        assert!(hook_socket_path().is_ok());
        assert!(resources_path().is_ok());
        assert!(backups_dir().is_ok());
        assert!(logs_dir().is_ok());
        assert!(settings_path().is_ok());
        assert!(themes_dir().is_ok());
    }

    /// If this test fails then either of these paths were changed.
    ///
    /// Since we set the permissions of the parent of these paths, make sure they're in folders we
    /// own otherwise we will set permissions of directories we shouldn't
    #[test]
    fn test_socket_paths() {
        assert_eq!(
            host_sockets_dir().unwrap().file_name().unwrap().to_str().unwrap(),
            RUNTIME_DIR_NAME
        );
    }

    macro_rules! assert_directory {
        ($value:expr, @$snapshot:literal) => {
            insta::assert_snapshot!(
                sanitized_directory_path($value),
                @$snapshot,
            )
        };
    }

    fn sanitized_directory_path(path: Result<PathBuf>) -> String {
        let mut path = path.unwrap().into_os_string().into_string().unwrap();

        if let Ok(home) = std::env::var("HOME") {
            let home = home.strip_suffix('/').unwrap_or(&home);
            path = path.replace(home, "$HOME");
        }

        let user = whoami::username().unwrap_or_else(|_| "unknown".into());
        path = path.replace(&user, "$USER");

        if let Ok(tmpdir) = std::env::var("TMPDIR") {
            let tmpdir = tmpdir.strip_suffix('/').unwrap_or(&tmpdir);
            path = path.replace(tmpdir, "$TMPDIR");
        }

        if let Ok(tmpdir) = macos_tempdir() {
            let tmpdir = tmpdir.to_str().unwrap();
            let tmpdir = tmpdir.strip_suffix('/').unwrap_or(tmpdir);
            path = path.replace(tmpdir, "$TMPDIR");
        };

        path
    }

    #[test]
    fn snapshot_home_local_bin() {
        assert_directory!(home_local_bin(), @"$HOME/.local/bin");
    }

    #[test]
    fn snapshot_fig_data_dir() {
        assert_directory!(fig_data_dir(), @"$HOME/Library/Application Support/fig");
    }

    #[test]
    fn snapshot_sockets_dir() {
        assert_directory!(sockets_dir(), @"$TMPDIR/figrun");
    }

    #[test]
    fn snapshot_themes_dir() {
        assert_directory!(themes_dir(), @"/Applications/Fig.app/Contents/Resources/themes");
    }

    #[test]
    fn snapshot_backups_dir() {
        assert_directory!(backups_dir(), @"$HOME/.fig.dotfiles.bak");
    }

    #[test]
    fn snapshot_fig_socket_path() {
        assert_directory!(desktop_socket_path(), @"$TMPDIR/figrun/desktop.sock");
    }

    #[test]
    fn snapshot_hook_socket_path() {
        assert_directory!(hook_socket_path(), @"$TMPDIR/figrun/hook.sock");
    }

    #[test]
    fn snapshot_settings_path() {
        assert_directory!(settings_path(), @"$HOME/Library/Application Support/fig/settings.json");
    }

    #[test]
    fn socket_path_length() {
        use std::os::unix::ffi::OsStrExt;
        /// Sockets are bounded at 100 bytes, why, because legacy compat
        const MAX_SOCKET_LEN: usize = 100;

        let fig_socket = desktop_socket_path().unwrap();
        let fig_socket_bytes = fig_socket.as_os_str().as_bytes().len();
        assert!(fig_socket_bytes <= MAX_SOCKET_LEN);

        let hook_socket = hook_socket_path().unwrap();
        assert!(hook_socket.as_os_str().as_bytes().len() <= MAX_SOCKET_LEN);
    }

    #[test]
    fn macos_tempdir_test() {
        let tmpdir = macos_tempdir().unwrap();
        println!("{:?}", tmpdir);
    }
}
