pub const APP_BUNDLE_ID: &str = env!("FIG_BRAND_BUNDLE_ID");
pub const APP_BUNDLE_NAME: &str = concat!(env!("FIG_BRAND_APP_NAME"), ".app");

pub const APP_PROCESS_NAME: &str = env!("FIG_BRAND_DESKTOP_BIN");

pub const CLI_BINARY_NAME: &str = env!("FIG_BRAND_CLI_BIN");

pub const CLI_CRATE_NAME: &str = "fig_cli";

pub const URL_SCHEMA: &str = env!("FIG_BRAND_URL_SCHEME");

pub const PRODUCT_NAME: &str = env!("FIG_BRAND_APP_NAME");

pub const RUNTIME_DIR_NAME: &str = env!("FIG_BRAND_RUNTIME_DIR_NAME");

/// Data directory name used in paths like ~/.local/share/{DATA_DIR_NAME}
pub const DATA_DIR_NAME: &str = env!("FIG_BRAND_DATA_DIR_NAME");

/// Backup directory name
pub(crate) const BACKUP_DIR_NAME: &str = env!("FIG_BRAND_BACKUP_DIR_NAME");

/// Label of the LaunchAgent that starts the desktop app at login.
///
/// Derived from [`APP_BUNDLE_ID`] so a bundle id change moves the plist with it; the
/// uninstaller sweeps `~/Library/LaunchAgents` by the `{APP_BUNDLE_ID}.` prefix.
pub const LAUNCH_AGENT_LABEL: &str = concat!(env!("FIG_BRAND_BUNDLE_ID"), ".launcher");

/// Prefix the uninstaller uses to find every LaunchAgent this app wrote.
pub const LAUNCH_AGENT_PREFIX: &str = concat!(env!("FIG_BRAND_BUNDLE_ID"), ".");

/// Version of the zsh hook's line protocol (`fig.zsh` <-> `fig_shell_ipc::hook`).
///
/// A shell sources `fig.zsh` once and keeps it for its whole life, so a tab opened before a
/// rebuild talks to the new desktop with the old script. `HELLO` carries this number and the
/// desktop answers `WELCOME 0` to any other, which disarms the hook in that shell. Bump it with
/// any change to the wire format that an older script or desktop would misread.
pub const HOOK_PROTOCOL_VERSION: u32 = 1;

/// The one line anyone may send `hook.sock` instead of `HELLO` to learn whether the desktop is
/// there: it answers `PONG <HOOK_PROTOCOL_VERSION>` and hangs up, and no session is created.
pub const HOOK_PING: &str = "PING";

/// Build time env vars
pub mod build {
    /// A git full sha hash of the current build
    pub const HASH: Option<&str> = option_env!("FIG_BUILD_HASH");

    /// The datetime in rfc3339 format of the current build
    pub const DATETIME: Option<&str> = option_env!("FIG_BUILD_DATETIME");
}

/// macOS specific constants
pub mod macos {
    pub const BUNDLE_CONTENTS_MACOS_PATH: &str = "Contents/MacOS";
    pub(crate) const BUNDLE_CONTENTS_RESOURCE_PATH: &str = "Contents/Resources";
    pub const BUNDLE_CONTENTS_INFO_PLIST_PATH: &str = "Contents/Info.plist";
}

pub mod env_var {
    macro_rules! define_env_vars {
        ($($(#[$meta:meta])* $ident:ident = $name:expr),*) => {
            $(
                $(#[$meta])*
                pub const $ident: &str = $name;
            )*

            pub const ALL: &[&str] = &[$($ident),*];
        }
    }

    define_env_vars! {
        /// Sets the current log level
        FIG_LOG_LEVEL = "FIG_LOG_LEVEL",

        /// Overrides the ZDOTDIR environment variable
        FIG_ZDOTDIR = "FIG_ZDOTDIR",

        /// Indicates a process was launched by the desktop app
        PROCESS_LAUNCHED_BY_FIG = "PROCESS_LAUNCHED_BY_FIG",

        /// The shell to use in the pty wrapper
        FIG_SHELL = "FIG_SHELL",

        /// Indicates the user is using zsh autosuggestions which disables Inline
        FIG_USING_ZSH_AUTOSUGGESTIONS = "FIG_USING_ZSH_AUTOSUGGESTIONS"
    }
}

pub mod system_paths {
    /// System installation paths
    pub const APPLICATIONS_DIR: &str = "/Applications";
    pub const USR_LOCAL_BIN: &str = "/usr/local/bin";
    pub const OPT_HOMEBREW_BIN: &str = "/opt/homebrew/bin";
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    use super::*;

    #[test]
    fn test_build_envs() {
        if let Some(build_hash) = build::HASH {
            println!("build_hash: {build_hash}");
            assert!(!build_hash.is_empty());
        }

        if let Some(build_datetime) = build::DATETIME {
            println!("build_datetime: {build_datetime}");
            println!("{}", OffsetDateTime::parse(build_datetime, &Rfc3339).unwrap());
        }
    }
}
