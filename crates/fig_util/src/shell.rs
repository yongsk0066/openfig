use std::fmt::Display;
use std::path::{
    Path,
    PathBuf,
};
use std::str::FromStr;
use std::sync::LazyLock;

use clap::ValueEnum;
use fig_os_shim::Env;
use regex::Regex;
use serde::{
    Deserialize,
    Serialize,
};
use tokio::process::Command;

use crate::env_var::FIG_ZDOTDIR;
use crate::{
    Error,
    directories,
    get_parent_process_exe,
};

/// All supported shells
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum Shell {
    /// Bash shell
    Bash,
    /// Zsh shell
    Zsh,
}

impl Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Shell {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "bash" => Ok(Shell::Bash),
            "zsh" => Ok(Shell::Zsh),
            _ => Err(()),
        }
    }
}

impl Shell {
    pub fn all() -> &'static [Self] {
        &[Shell::Bash, Shell::Zsh]
    }

    /// All shells to run unit / integration tests with
    pub fn all_test() -> Vec<Self> {
        vec![Shell::Bash, Shell::Zsh]
    }

    /// Try to find the name of common shells in the input
    pub fn try_find_shell(input: impl AsRef<Path>) -> Option<Self> {
        let input = input.as_ref().file_name()?.to_str()?;
        if input.contains("bash") {
            Some(Shell::Bash)
        } else if input.contains("zsh") {
            Some(Shell::Zsh)
        } else {
            None
        }
    }

    /// Gets the current shell of the parent process
    pub fn current_shell() -> Option<Self> {
        let parent_exe = get_parent_process_exe()?;
        let parent_exe_name = parent_exe.to_str()?;
        Self::try_find_shell(parent_exe_name)
    }

    pub async fn current_shell_version() -> Result<(Self, String), Error> {
        let parent_exe = get_parent_process_exe().ok_or(Error::NoParentProcess)?;
        let Some(shell) = Self::try_find_shell(&parent_exe) else {
            return Err(Error::UnknownShell(parent_exe.to_string_lossy().into()));
        };

        Ok((shell, shell_version(&shell, &parent_exe).await?))
    }

    /// Get the directory for the shell that contains the dotfiles
    ///
    /// The home fallback goes through `env`, not `directories::home_dir()`. The latter reads the
    /// *process* environment, so an injected [`Env`] was ignored on exactly the path that decides
    /// which files get rewritten — which meant every test of the integration code silently
    /// operated on the developer's own dotfiles.
    pub fn get_config_directory(&self, env: &Env) -> Result<PathBuf, directories::DirectoryError> {
        let home = || env.home().ok_or(directories::DirectoryError::NoHomeDirectory);
        match self {
            Shell::Bash => home(),
            Shell::Zsh => match env
                .get_os("ZDOTDIR")
                .or_else(|| env.get_os(FIG_ZDOTDIR))
                .map(PathBuf::from)
            {
                Some(dir) => Ok(dir),
                None => home(),
            },
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
        }
    }
}

/// Matches the first line of `bash --version`.
///
/// The words between `GNU bash,` and the version number are localized (`version` in the C locale,
/// `\uBC84\uC804` under `LANG=ko_KR`), so anything non-numeric is skipped instead of being matched
/// literally. `shell_version` also forces `LC_ALL=C`; this is the second line of defence.
static BASH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^GNU bash,\D*(\d+\.\d+\.\d+)").unwrap());

/// Matches the first line of `zsh --version`, e.g. `zsh 5.9 (arm64-apple-darwin26.0)` or
/// `zsh 5.9.2 (aarch64-apple-darwin25.4.0)`. The patch component is optional but must not be
/// truncated when it is present.
static ZSH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^zsh\s+(\d+\.\d+(?:\.\d+)?)").unwrap());

async fn shell_version(shell: &Shell, exe_path: &Path) -> Result<String, Error> {
    let err = || Error::ShellVersion(*shell);
    let re: &Regex = match shell {
        Shell::Bash => &BASH_RE,
        Shell::Zsh => &ZSH_RE,
    };
    // `LC_ALL=C` keeps the banner in English regardless of the user's locale.
    let version_output = Command::new(exe_path)
        .arg("--version")
        .env("LC_ALL", "C")
        .output()
        .await?;
    let version_capture = re.captures(std::str::from_utf8(&version_output.stdout)?);
    Ok(version_capture.ok_or_else(err)?.get(1).ok_or_else(err)?.as_str().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shell_version() {
        // Every entry must resolve to a `major.minor[.patch]` triple. `bash` resolves through
        // `$PATH` (Homebrew bash 5.x on this machine), `/bin/bash` is the Apple-shipped 3.2.57.
        let tests = [
            (Shell::Bash, "/bin/bash"),
            (Shell::Bash, "bash"),
            (Shell::Zsh, "/bin/zsh"),
        ];

        for (shell, exe_path_str) in tests {
            let exe_path = Path::new(exe_path_str);
            let version: String = shell_version(&shell, exe_path)
                .await
                .unwrap_or_else(|err| panic!("exe {exe_path_str} failed. Error: {err:?}"));

            let mut parts = version.split('.');
            let major: u32 = parts
                .next()
                .and_then(|p| p.parse().ok())
                .unwrap_or_else(|| panic!("{exe_path_str}: no major in {version:?}"));
            let minor = parts
                .next()
                .unwrap_or_else(|| panic!("{exe_path_str}: no minor in {version:?}"));
            assert!(
                minor.parse::<u32>().is_ok(),
                "{exe_path_str}: minor {minor:?} of {version:?} is not numeric"
            );
            if let Some(patch) = parts.next() {
                assert!(
                    patch.parse::<u32>().is_ok(),
                    "{exe_path_str}: patch {patch:?} of {version:?} is not numeric"
                );
            }
            assert!(parts.next().is_none(), "{exe_path_str}: trailing junk in {version:?}");
            assert!(major >= 3, "{exe_path_str}: implausible major {major} in {version:?}");
        }

        // `/bin/bash` is pinned by macOS and will not move off 3.2.57.
        assert_eq!(
            shell_version(&Shell::Bash, Path::new("/bin/bash")).await.unwrap(),
            "3.2.57"
        );
    }

    #[test]
    fn test_bash_re() {
        let bash_3_version = indoc::indoc! {r"
            GNU bash, version 3.2.57(1)-release (x86_64-apple-darwin23)
            Copyright (C) 2007 Free Software Foundation, Inc.
        "};
        assert_eq!(
            BASH_RE.captures(bash_3_version).unwrap().get(1).unwrap().as_str(),
            "3.2.57"
        );

        let bash_5_version = indoc::indoc! {r"
            GNU bash, version 5.2.26(1)-release (aarch64-apple-darwin23.2.0)
            Copyright (C) 2022 Free Software Foundation, Inc.
            License GPLv3+: GNU GPL version 3 or later <http://gnu.org/licenses/gpl.html>

            This is free software; you are free to change and redistribute it.
            There is NO WARRANTY, to the extent permitted by law.
        "};
        assert_eq!(
            BASH_RE.captures(bash_5_version).unwrap().get(1).unwrap().as_str(),
            "5.2.26"
        );
    }

    /// Homebrew bash 5.3+ is built with NLS, so under `LANG=ko_KR` the banner reads
    /// `GNU bash, 버전 5.3.15(1)-release`. The old `GNU bash, version …` pattern missed it and
    /// `test_shell_version` failed on every run.
    #[test]
    fn test_bash_re_localized() {
        let ko = indoc::indoc! {r"
            GNU bash, 버전 5.3.15(1)-release (aarch64-apple-darwin25.4.0)
            Copyright (C) 2025 Free Software Foundation, Inc.
            라이선스 GPLv3+: GNU GPL 버전 3 이상 <http://gnu.org/license/gpl.html>
        "};
        assert_eq!(BASH_RE.captures(ko).unwrap().get(1).unwrap().as_str(), "5.3.15");

        // Other NLS banners take the same shape.
        assert_eq!(
            BASH_RE
                .captures("GNU bash, Version 5.1.16(1)-release")
                .unwrap()
                .get(1)
                .unwrap()
                .as_str(),
            "5.1.16"
        );

        // A line that merely mentions a version elsewhere must not match.
        assert!(BASH_RE.captures("License GPLv3+: GNU GPL version 3 or later").is_none());
    }

    #[test]
    fn test_zsh_re() {
        // `/bin/zsh` has no patch component, Homebrew's does. Neither may be truncated.
        assert_eq!(
            ZSH_RE
                .captures("zsh 5.9 (arm-apple-darwin22.1.0)")
                .unwrap()
                .get(1)
                .unwrap()
                .as_str(),
            "5.9"
        );
        assert_eq!(
            ZSH_RE
                .captures("zsh 5.9.2 (aarch64-apple-darwin25.4.0)")
                .unwrap()
                .get(1)
                .unwrap()
                .as_str(),
            "5.9.2"
        );
        // The old `(\d+\.\d+)` pattern matched the first number anywhere, e.g. in the target triple.
        assert!(ZSH_RE.captures("arm-apple-darwin22.1.0").is_none());
    }
}
