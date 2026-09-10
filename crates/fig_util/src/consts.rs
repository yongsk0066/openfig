pub const APP_BUNDLE_ID: &str = "com.amazon.codewhisperer";
pub const APP_BUNDLE_NAME: &str = "Amazon Q.app";

pub const APP_PROCESS_NAME: &str = "q_desktop";

/// The name configured under `"package.productName"` in the tauri.conf.json file.
pub const TAURI_PRODUCT_NAME: &str = "q_desktop";

pub const CLI_BINARY_NAME: &str = "q";
pub const PTY_BINARY_NAME: &str = "qterm";

pub const CLI_CRATE_NAME: &str = "q_cli";

pub const URL_SCHEMA: &str = "q";

pub const PRODUCT_NAME: &str = "Amazon Q";

pub const RUNTIME_DIR_NAME: &str = "cwrun";

/// Data directory name used in paths like ~/.local/share/{DATA_DIR_NAME}
pub const DATA_DIR_NAME: &str = "amazon-q";

/// Backup directory name
pub const BACKUP_DIR_NAME: &str = ".amazon-q.dotfiles.bak";

// These are the old "CodeWhisperer" branding, used anywhere we will not update to Amazon Q
pub const OLD_PRODUCT_NAME: &str = "CodeWhisperer";
pub const OLD_CLI_BINARY_NAMES: &[&str] = &["cw"];
pub const OLD_PTY_BINARY_NAMES: &[&str] = &["cwterm"];

/// Build time env vars
pub mod build {
    /// The target of the current build, e.g. "aarch64-unknown-linux-musl"
    pub const TARGET_TRIPLE: Option<&str> = option_env!("AMAZON_Q_BUILD_TARGET_TRIPLE");

    /// The variant of the current build
    pub const VARIANT: Option<&str> = option_env!("AMAZON_Q_BUILD_VARIANT");

    /// A git full sha hash of the current build
    pub const HASH: Option<&str> = option_env!("AMAZON_Q_BUILD_HASH");

    /// The datetime in rfc3339 format of the current build
    pub const DATETIME: Option<&str> = option_env!("AMAZON_Q_BUILD_DATETIME");

    /// If `shellcheck` tests should be skipped
    pub const SKIP_SHELLCHECK_TESTS: bool = option_env!("AMAZON_Q_BUILD_SKIP_SHELLCHECK_TESTS").is_some();
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
        /// The UUID of the current parent qterm instance
        QTERM_SESSION_ID = "QTERM_SESSION_ID",

        /// Set if qterm is running, contains the version
        Q_TERM = "Q_TERM",

        /// Sets the current log level
        Q_LOG_LEVEL = "Q_LOG_LEVEL",

        /// Overrides the ZDOTDIR environment variable
        Q_ZDOTDIR = "Q_ZDOTDIR",

        /// Indicates a process was launched by Amazon Q
        PROCESS_LAUNCHED_BY_Q = "PROCESS_LAUNCHED_BY_Q",

        /// The shell to use in qterm
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
