use std::convert::TryInto;
use std::fmt::Display;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use fig_os_shim::{
    Context,
    EnvProvider,
    FsProvider,
    Shim,
};
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

pub fn home_dir_ctx<Ctx: FsProvider + EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    if ctx.env().is_real() {
        home_dir()
    } else {
        ctx.env()
            .get("HOME")
            .map_err(|_err| DirectoryError::NoHomeDirectory)
            .and_then(|h| {
                if h.is_empty() {
                    Err(DirectoryError::NoHomeDirectory)
                } else {
                    Ok(h)
                }
            })
            .map(PathBuf::from)
            .map(|p| ctx.fs().chroot_path(p))
    }
}

/// The directory of the users `$HOME/.local/bin` directory
///
/// MacOS path: `$HOME/.local/bin``
pub fn home_local_bin() -> Result<PathBuf> {
    let mut path = home_dir()?;
    path.push(".local/bin");
    Ok(path)
}

pub fn home_local_bin_ctx(ctx: &Context) -> Result<PathBuf> {
    let mut path = home_dir_ctx(ctx)?;
    path.push(".local/bin");
    Ok(path)
}

/// The config directory
///
/// - MacOS: `$HOME/Library/Application Support`
pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir().ok_or(DirectoryError::NoHomeDirectory)
}

/// The old codewhisperer data directory
///
/// This should be removed at some point in the future, once all our users have migrated
/// - MacOS: `$HOME/Library/Application Support/codewhisperer`
pub fn old_fig_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or(DirectoryError::NoHomeDirectory)?
        .join("codewhisperer"))
}

/// The q data directory
///
/// - MacOS: `$HOME/Library/Application Support/{data_dir}`
pub fn fig_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or(DirectoryError::NoHomeDirectory)?
        .join(DATA_DIR_NAME))
}

pub fn fig_data_dir_ctx(fs: &impl FsProvider) -> Result<PathBuf> {
    Ok(fs.fs().chroot_path(fig_data_dir()?))
}

/// The user's local data directory.
///
/// - MacOS: `$HOME/Library/Application Support`
pub fn local_data_dir<Ctx: FsProvider + EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    Ok(home_dir_ctx(ctx)?.join("Library/Application Support"))
}

/// The q cache directory
///
/// - MacOS: `$HOME/Library/Caches/{data_dir}`
pub fn cache_dir() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .ok_or(DirectoryError::NoHomeDirectory)?
        .join(DATA_DIR_NAME))
}

/// Get the macos tempdir from the `confstr` function
///
/// See: <https://man7.org/linux/man-pages/man3/confstr.3.html>
fn macos_tempdir() -> Result<PathBuf> {
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null::<i8>().cast_mut(), 0) };
    let mut buf: Vec<u8> = vec![0; len];
    unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    let c_string = std::ffi::CString::from_vec_with_nul(buf)?;
    let str = c_string.into_string()?;
    Ok(PathBuf::from(str))
}

/// Runtime dir is used for runtime data that should not be persisted for a long time, e.g. socket
/// files and logs
///
/// On macOS this falls back to TMPDIR, which is secure on macOS
pub fn runtime_dir() -> Result<PathBuf> {
    let mut dir = dirs::runtime_dir();
    dir = dir.or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from));
    dir = dir.or(Some(macos_tempdir()?));
    dir.ok_or(DirectoryError::NoRuntimeDirectory)
}

/// The q sockets directory of the local q installation
///
/// - MacOS: $TMPDIR/cwrun
pub fn sockets_dir() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(RUNTIME_DIR_NAME))
}

/// The directory on the host machine where socket files are stored
///
/// - MacOS: $TMPDIR/cwrun
pub fn host_sockets_dir() -> Result<PathBuf> {
    sockets_dir()
}

/// The path to all of the themes
pub fn themes_dir(ctx: &Context) -> Result<PathBuf> {
    Ok(resources_path_ctx(ctx)?.join("themes"))
}

/// The autocomplete directory
pub fn autocomplete_dir() -> Result<PathBuf> {
    Ok(fig_data_dir()?.join("autocomplete"))
}

/// The autocomplete specs directory
pub fn autocomplete_specs_dir() -> Result<PathBuf> {
    Ok(autocomplete_dir()?.join("specs"))
}

/// The directory to all the fig logs
/// - MacOS: `$TMPDIR/logs`
pub fn logs_dir() -> Result<PathBuf> {
    use crate::CLI_BINARY_NAME;
    Ok(runtime_dir()?.join(format!("{CLI_BINARY_NAME}log")))
}

/// The directory where fig places all data-sensitive backups
///
/// - MacOS: `$HOME/{backup_dir}`
pub fn backups_dir() -> Result<PathBuf> {
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

/// The directory to the directory containing config for the `/context` feature in `q chat`.
pub fn chat_global_context_path<Ctx: FsProvider + EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    Ok(home_dir_ctx(ctx)?
        .join(".aws")
        .join("amazonq")
        .join("global_context.json"))
}

/// The directory to the directory containing config for the `/context` feature in `q chat`.
pub fn chat_profiles_dir<Ctx: FsProvider + EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    Ok(home_dir_ctx(ctx)?.join(".aws").join("amazonq").join("profiles"))
}

/// The desktop app socket path
///
/// - MacOS: `$TMPDIR/cwrun/desktop.sock`
pub fn desktop_socket_path() -> Result<PathBuf> {
    Ok(host_sockets_dir()?.join("desktop.sock"))
}

/// The path to local remote socket
///
/// - MacOS: `$TMPDIR/cwrun/remote.sock`
pub fn local_remote_socket_path() -> Result<PathBuf> {
    Ok(host_sockets_dir()?.join("remote.sock"))
}

/// Get path to a figterm socket
///
/// - MacOS: `$TMPDIR/cwrun/t/$SESSION_ID.sock`
pub fn figterm_socket_path(session_id: impl Display) -> Result<PathBuf> {
    Ok(sockets_dir()?.join("t").join(format!("{session_id}.sock")))
}

/// The path to the resources directory
///
/// - MacOS: "/Applications/{app_name}.app/Contents/Resources"
pub fn resources_path() -> Result<PathBuf> {
    Ok(crate::app_bundle_path().join(crate::macos::BUNDLE_CONTENTS_RESOURCE_PATH))
}

pub fn resources_path_ctx<Ctx: EnvProvider>(_ctx: &Ctx) -> Result<PathBuf> {
    resources_path()
}

/// The path to the fig install manifest
///
/// - MacOS: "/Applications/{app_name}.app/Contents/Resources/manifest.json"
pub fn manifest_path() -> Result<PathBuf> {
    Ok(resources_path()?.join("manifest.json"))
}

/// The path to the fig settings file
///
/// - MacOS: `$HOME/Library/Application Support/{data_dir}/settings.json`
pub fn settings_path() -> Result<PathBuf> {
    Ok(fig_data_dir()?.join("settings.json"))
}

/// The path to the midway cookie
///
/// Path: `$HOME/.midway/cookie`
pub fn midway_cookie_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".midway").join("cookie"))
}

utf8_dir!(home_dir);
utf8_dir!(home_local_bin);
utf8_dir!(fig_data_dir);
utf8_dir!(sockets_dir);
utf8_dir!(figterm_socket_path, session_id: impl Display);
utf8_dir!(manifest_path);
utf8_dir!(backups_dir);
utf8_dir!(logs_dir);
utf8_dir!(settings_path);

#[cfg(test)]
mod tests {
    use insta;

    use super::*;

    #[test]
    fn all_paths() {
        let ctx = Context::new();
        assert!(home_dir().is_ok());
        assert!(home_local_bin().is_ok());
        assert!(fig_data_dir().is_ok());
        assert!(sockets_dir().is_ok());
        assert!(local_remote_socket_path().is_ok());
        assert!(figterm_socket_path("test").is_ok());
        assert!(resources_path().is_ok());
        assert!(manifest_path().is_ok());
        assert!(backups_dir().is_ok());
        assert!(logs_dir().is_ok());
        assert!(settings_path().is_ok());
        assert!(midway_cookie_path().is_ok());
        assert!(themes_dir(&ctx).is_ok());
    }

    /// If this test fails then either of these paths were changed.
    ///
    /// Since we set the permissions of the parent of these paths, make sure they're in folders we
    /// own otherwise we will set permissions of directories we shouldn't
    #[test]
    fn test_socket_paths() {
        assert_eq!(
            host_sockets_dir().unwrap().file_name().unwrap().to_str().unwrap(),
            format!("cwrun")
        );

        assert_eq!(
            figterm_socket_path("").unwrap().parent().unwrap().file_name().unwrap(),
            "t"
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

        let user = whoami::username();
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
        assert_directory!(fig_data_dir(), @"$HOME/Library/Application Support/amazon-q");
    }

    #[test]
    fn snapshot_sockets_dir() {
        assert_directory!(sockets_dir(), @"$TMPDIR/cwrun");
    }

    #[test]
    fn snapshot_themes_dir() {
        assert_directory!(themes_dir(&Context::new()), @"/Applications/Amazon Q.app/Contents/Resources/themes");
    }

    #[test]
    fn snapshot_backups_dir() {
        assert_directory!(backups_dir(), @"$HOME/.amazon-q.dotfiles.bak");
    }

    #[test]
    fn snapshot_fig_socket_path() {
        assert_directory!(desktop_socket_path(), @"$TMPDIR/cwrun/desktop.sock");
    }

    #[test]
    fn snapshot_local_remote_socket_path() {
        assert_directory!(local_remote_socket_path(), @"$TMPDIR/cwrun/remote.sock");
    }

    #[test]
    fn snapshot_figterm_socket_path() {
        assert_directory!(figterm_socket_path("$SESSION_ID"), @"$TMPDIR/cwrun/t/$SESSION_ID.sock");
    }

    #[test]
    fn snapshot_settings_path() {
        assert_directory!(settings_path(), @"$HOME/Library/Application Support/amazon-q/settings.json");
    }

    #[test]
    fn snapshot_midway_cookie_path() {
        assert_directory!(midway_cookie_path(), @"$HOME/.midway/cookie");
    }

    #[test]
    fn socket_path_length() {
        use std::os::unix::ffi::OsStrExt;
        /// Sockets are bounded at 100 bytes, why, because legacy compat
        const MAX_SOCKET_LEN: usize = 100;

        let uuid = uuid::Uuid::new_v4().simple().to_string();
        let qterm_socket = figterm_socket_path(uuid.clone()).unwrap();
        let qterm_socket_bytes = qterm_socket.as_os_str().as_bytes().len();
        assert!(qterm_socket_bytes <= MAX_SOCKET_LEN);

        let fig_socket = desktop_socket_path().unwrap();
        let fig_socket_bytes = fig_socket.as_os_str().as_bytes().len();
        assert!(fig_socket_bytes <= MAX_SOCKET_LEN);

        let secure_socket = local_remote_socket_path().unwrap();
        let secure_socket_bytes = secure_socket.as_os_str().as_bytes().len();
        assert!(secure_socket_bytes <= MAX_SOCKET_LEN);
    }

    #[test]
    fn macos_tempdir_test() {
        let tmpdir = macos_tempdir().unwrap();
        println!("{:?}", tmpdir);
    }
}
