use std::borrow::Cow;
use std::sync::OnceLock;

use fig_os_shim::Env;
use serde::{
    Deserialize,
    Serialize,
};

/// The support level for different platforms
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportLevel {
    /// A fully supported platform
    Supported,
    /// Supported, but with a caveat
    SupportedWithCaveat { info: Cow<'static, str> },
    /// A platform that is currently in development
    InDevelopment { info: Option<Cow<'static, str>> },
    /// A platform that is not supported
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OSVersion {
    MacOS {
        major: i32,
        minor: i32,
        patch: Option<i32>,
        build: String,
    },
}

impl OSVersion {
    pub fn support_level(&self) -> SupportLevel {
        match self {
            OSVersion::MacOS { major, minor, .. } => {
                // Minimum supported macOS version is 10.14.0
                if *major > 10 || (*major == 10 && *minor >= 14) {
                    SupportLevel::Supported
                } else {
                    SupportLevel::Unsupported
                }
            },
        }
    }
}

impl std::fmt::Display for OSVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OSVersion::MacOS {
                major,
                minor,
                patch,
                build,
            } => {
                let patch = patch.unwrap_or(0);
                write!(f, "macOS {major}.{minor}.{patch} ({build})")
            },
        }
    }
}

pub fn os_version() -> Option<&'static OSVersion> {
    static OS_VERSION: OnceLock<Option<OSVersion>> = OnceLock::new();
    OS_VERSION
        .get_or_init(|| {
            use std::process::Command;

            use regex::Regex;

            let version_info = Command::new("sw_vers").output().ok()?;

            let version_info: String = String::from_utf8_lossy(&version_info.stdout).trim().into();

            let version_regex = Regex::new(r"ProductVersion:\s*(\S+)").unwrap();
            let build_regex = Regex::new(r"BuildVersion:\s*(\S+)").unwrap();

            let version: String = version_regex
                .captures(&version_info)
                .and_then(|c| c.get(1))
                .map(|v| v.as_str().into())?;

            let major = version.split('.').next()?.parse().ok()?;

            let minor = version.split('.').nth(1)?.parse().ok()?;

            let patch = version.split('.').nth(2).and_then(|p| p.parse().ok());

            let build = build_regex
                .captures(&version_info)
                .and_then(|c| c.get(1))?
                .as_str()
                .into();

            Some(OSVersion::MacOS {
                major,
                minor,
                patch,
                build,
            })
        })
        .as_ref()
}

pub fn in_ssh() -> bool {
    static IN_SSH: OnceLock<bool> = OnceLock::new();
    *IN_SSH.get_or_init(|| Env::new().in_ssh())
}

/// Is the calling binary running on a remote instance
pub fn is_remote() -> bool {
    // TODO(chay): Add detection for inside docker container
    in_ssh() || fig_os_shim::Env::new().fig_fake_is_remote()
}

pub fn in_codespaces() -> bool {
    static IN_CODESPACES: OnceLock<bool> = OnceLock::new();
    *IN_CODESPACES.get_or_init(|| fig_os_shim::Env::new().in_codespaces())
}

pub fn in_ci() -> bool {
    static IN_CI: OnceLock<bool> = OnceLock::new();
    *IN_CI.get_or_init(|| fig_os_shim::Env::new().in_ci())
}
