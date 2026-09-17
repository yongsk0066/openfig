use std::borrow::Cow;
use std::path::{
    Path,
    PathBuf,
};
use std::time::Duration;

use async_trait::async_trait;
use fig_integrations::shell::{
    ZSH_HOOK_MIN_VERSION,
    ZSH_HOOK_SETTING,
};
use fig_util::env_var::FIG_TERM;
use fig_util::{
    CLI_BINARY_NAME,
    HOOK_PING,
    HOOK_PROTOCOL_VERSION,
    PTY_BINARY_NAME,
    Shell,
};
use owo_colors::OwoColorize;
use tokio::io::{
    AsyncBufReadExt as _,
    AsyncWriteExt as _,
    BufReader,
};

use crate::cli::doctor::{
    DoctorCheck,
    DoctorCheckType,
    DoctorError,
    Platform,
};

const FORCE_FIGTERM: &str = "FIG_FORCE_FIGTERM_LAUNCH";

/// How a new zsh gets autocomplete (docs/15-architecture-study.md §3.3): the decision `init zsh
/// pre` and `post` make, taken with the same inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ZshPath {
    /// `fig.zsh` over `hook.sock`.
    Hook,
    /// The pty binary: the setting is off, or forced.
    Figterm { forced: bool },
    /// The setting is on and zsh is too old for the hook: no autocomplete at all.
    TooOld,
}

fn decide(setting: bool, forced: bool, version: Option<&str>) -> ZshPath {
    if forced || !setting {
        return ZshPath::Figterm { forced };
    }
    match version {
        Some(version) if !at_least(version, ZSH_HOOK_MIN_VERSION) => ZshPath::TooOld,
        _ => ZshPath::Hook,
    }
}

/// zsh's `is-at-least`, for plain dotted numbers.
fn at_least(version: &str, min: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> {
        v.split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    parts(version) >= parts(min)
}

/// What `hook.sock` said to `PING`: its protocol version.
async fn ping(socket: &Path) -> Result<u32, String> {
    let exchange = async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|err| format!("cannot connect to {}: {err}", socket.display()))?;
        let (reader, mut writer) = stream.into_split();
        writer
            .write_all(format!("{HOOK_PING}\n").as_bytes())
            .await
            .map_err(|err| format!("write failed: {err}"))?;
        let line = BufReader::new(reader)
            .lines()
            .next_line()
            .await
            .map_err(|err| format!("read failed: {err}"))?
            .ok_or("hung up without an answer")?;
        line.strip_prefix("PONG ")
            .and_then(|version| version.parse().ok())
            .ok_or_else(|| format!("unexpected answer {line:?}"))
    };
    tokio::time::timeout(Duration::from_secs(2), exchange)
        .await
        .map_err(|_elapsed| "no answer within 2 s".to_owned())?
}

/// The zsh a new terminal tab starts: the login shell when it is zsh, else the one on `PATH`.
fn login_zsh() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| Shell::try_find_shell(shell) == Some(Shell::Zsh))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

async fn zsh_version(zsh: &Path) -> Option<String> {
    let output = tokio::process::Command::new(zsh)
        .args(["-fc", "print -r -- $ZSH_VERSION"])
        .output()
        .await
        .ok()?;
    let version = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!version.is_empty()).then_some(version)
}

/// Whether the shell `doctor` runs in reports through the hook, so the pty binary's socket check
/// does not apply to it.
pub fn current_shell_uses_hook() -> bool {
    Shell::current_shell() == Some(Shell::Zsh)
        && std::env::var_os(FIG_TERM).is_none()
        && fig_settings::settings::get_bool_or(ZSH_HOOK_SETTING, false)
}

/// Which path a new zsh takes, which zsh that is, and whether `hook.sock` answers.
pub struct ZshHookCheck {
    zsh: PathBuf,
    version: Option<String>,
    path: ZshPath,
    socket: Result<u32, String>,
}

impl ZshHookCheck {
    pub async fn probe() -> Self {
        let zsh = login_zsh();
        let version = zsh_version(&zsh).await;
        let setting = fig_settings::settings::get_bool_or(ZSH_HOOK_SETTING, false);
        let path = decide(setting, std::env::var_os(FORCE_FIGTERM).is_some(), version.as_deref());
        let socket = match fig_util::directories::hook_socket_path() {
            Ok(socket) => ping(&socket).await,
            Err(err) => Err(err.to_string()),
        };
        ZshHookCheck {
            zsh,
            version,
            path,
            socket,
        }
    }
}

#[async_trait]
impl DoctorCheck for ZshHookCheck {
    fn name(&self) -> Cow<'static, str> {
        let path = match self.path {
            ZshPath::Hook => "the zsh hook".to_owned(),
            ZshPath::Figterm { forced: false } => format!("{PTY_BINARY_NAME} ({ZSH_HOOK_SETTING} is off)"),
            ZshPath::Figterm { forced: true } => format!("{PTY_BINARY_NAME} ({FORCE_FIGTERM})"),
            ZshPath::TooOld => "no autocomplete".to_owned(),
        };
        let socket = match &self.socket {
            Ok(version) => format!("hook.sock answers, protocol {version}"),
            Err(_) => "hook.sock does not answer".to_owned(),
        };
        format!(
            "New zsh shells use {path} (zsh {} at {}; {socket})",
            self.version.as_deref().unwrap_or("?"),
            self.zsh.display()
        )
        .into()
    }

    async fn get_type(&self, _: &(), platform: Platform) -> DoctorCheckType {
        if platform == Platform::MacOs {
            DoctorCheckType::NormalCheck
        } else {
            DoctorCheckType::NoCheck
        }
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        match (&self.path, &self.socket) {
            (ZshPath::TooOld, _) => Err(DoctorError::warning(format!(
                "{ZSH_HOOK_SETTING} is on but zsh {} is older than {ZSH_HOOK_MIN_VERSION}: new zsh shells get no autocomplete. Turn it off with {}",
                self.version.as_deref().unwrap_or("?"),
                format!("{CLI_BINARY_NAME} settings {ZSH_HOOK_SETTING} false").magenta()
            ))),
            (ZshPath::Hook, Err(reason)) => Err(DoctorError::Error {
                reason: format!("new zsh shells use the hook, but hook.sock does not answer: {reason}").into(),
                info: vec!["They run without autocomplete until the app is running.".into()],
                fix: None,
                error: None,
            }),
            (ZshPath::Hook, Ok(version)) if *version != HOOK_PROTOCOL_VERSION => Err(DoctorError::warning(format!(
                "hook.sock speaks protocol {version} and this {CLI_BINARY_NAME} speaks {HOOK_PROTOCOL_VERSION}: new zsh shells will be refused until the app and the CLI are from the same build"
            ))),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_follows_the_setting_the_force_and_the_version() {
        assert_eq!(decide(false, false, Some("5.9")), ZshPath::Figterm { forced: false });
        assert_eq!(decide(true, true, Some("5.9")), ZshPath::Figterm { forced: true });
        assert_eq!(decide(true, false, Some("5.9")), ZshPath::Hook);
        assert_eq!(decide(true, false, Some("5.3")), ZshPath::Hook);
        assert_eq!(decide(true, false, Some("5.2")), ZshPath::TooOld);
        assert_eq!(decide(true, false, Some("5.0.8")), ZshPath::TooOld);
        assert_eq!(decide(true, false, Some("5.10-dev-0")), ZshPath::Hook);
        // An unknown version is not held against the shell: `fig.zsh` checks again itself.
        assert_eq!(decide(true, false, None), ZshPath::Hook);
    }

    #[tokio::test]
    async fn a_missing_socket_does_not_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ping(&dir.path().join("hook.sock")).await.is_err());
    }

    #[tokio::test]
    async fn ping_reads_the_protocol_version_from_pong() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("hook.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let line = BufReader::new(reader).lines().next_line().await.unwrap().unwrap();
            assert_eq!(line, "PING");
            writer
                .write_all(format!("PONG {HOOK_PROTOCOL_VERSION}\n").as_bytes())
                .await
                .unwrap();
        });
        assert_eq!(ping(&socket).await, Ok(HOOK_PROTOCOL_VERSION));
    }
}
