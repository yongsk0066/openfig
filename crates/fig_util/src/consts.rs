pub const APP_BUNDLE_ID: &str = env!("FIG_BRAND_BUNDLE_ID");
pub const APP_BUNDLE_NAME: &str = concat!(env!("FIG_BRAND_APP_NAME"), ".app");

pub const APP_PROCESS_NAME: &str = env!("FIG_BRAND_DESKTOP_BIN");

pub const CLI_BINARY_NAME: &str = env!("FIG_BRAND_CLI_BIN");
pub const PTY_BINARY_NAME: &str = env!("FIG_BRAND_PTY_BIN");

pub const CLI_CRATE_NAME: &str = "fig_cli";

pub const URL_SCHEMA: &str = env!("FIG_BRAND_URL_SCHEME");

pub const PRODUCT_NAME: &str = env!("FIG_BRAND_APP_NAME");

pub const RUNTIME_DIR_NAME: &str = env!("FIG_BRAND_RUNTIME_DIR_NAME");

/// Data directory name used in paths like ~/.local/share/{DATA_DIR_NAME}
pub const DATA_DIR_NAME: &str = env!("FIG_BRAND_DATA_DIR_NAME");

/// Backup directory name
pub const BACKUP_DIR_NAME: &str = env!("FIG_BRAND_BACKUP_DIR_NAME");

/// Build time env vars
pub mod build {
    /// The target of the current build, e.g. "aarch64-unknown-linux-musl"
    pub const TARGET_TRIPLE: Option<&str> = option_env!("FIG_BUILD_TARGET_TRIPLE");

    /// A git full sha hash of the current build
    pub const HASH: Option<&str> = option_env!("FIG_BUILD_HASH");

    /// The datetime in rfc3339 format of the current build
    pub const DATETIME: Option<&str> = option_env!("FIG_BUILD_DATETIME");
}

/// macOS specific constants
pub mod macos {
    pub const BUNDLE_CONTENTS_MACOS_PATH: &str = "Contents/MacOS";
    pub const BUNDLE_CONTENTS_RESOURCE_PATH: &str = "Contents/Resources";
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
        /// The UUID of the current parent pty session
        QTERM_SESSION_ID = "QTERM_SESSION_ID",

        /// Set if the pty wrapper is running, contains the version
        Q_TERM = "Q_TERM",

        /// Sets the current log level
        Q_LOG_LEVEL = "Q_LOG_LEVEL",

        /// Overrides the ZDOTDIR environment variable
        Q_ZDOTDIR = "Q_ZDOTDIR",

        /// Indicates a process was launched by the desktop app
        PROCESS_LAUNCHED_BY_Q = "PROCESS_LAUNCHED_BY_Q",

        /// The shell to use in the pty wrapper
        Q_SHELL = "Q_SHELL",

        /// Indicates the user is debugging the shell
        Q_DEBUG_SHELL = "Q_DEBUG_SHELL",

        /// Indicates the user is using zsh autosuggestions which disables Inline
        Q_USING_ZSH_AUTOSUGGESTIONS = "Q_USING_ZSH_AUTOSUGGESTIONS"
    }
}

pub mod system_paths {
    /// System installation paths
    pub const APPLICATIONS_DIR: &str = "/Applications";
    pub const USR_LOCAL_BIN: &str = "/usr/local/bin";
    pub const USR_SHARE: &str = "/usr/share";
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
