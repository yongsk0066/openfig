mod checks;

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt::Display;
use std::future::Future;
use std::path::{
    Path,
    PathBuf,
};
use std::process::{
    Command,
    ExitCode,
};
use std::time::Duration;

use anstream::{
    eprintln,
    println,
};
use async_trait::async_trait;
use checks::ZshHookCheck;
use clap::Args;
use crossterm::style::Stylize;
use crossterm::terminal::{
    Clear,
    ClearType,
};
use crossterm::{
    cursor,
    execute,
};
use eyre::{
    ContextCompat,
    Result,
    WrapErr,
};
use fig_integrations::Error as InstallationError;
use fig_integrations::shell::{
    ShellExt,
    ShellIntegration,
};
use fig_os_shim::{
    Context,
    Env,
};
use fig_proto::local::DiagnosticsResponse;
use fig_settings::JsonStore;
use fig_util::directories::settings_path;
use fig_util::env_var::PROCESS_LAUNCHED_BY_FIG;
use fig_util::{
    APP_BUNDLE_NAME,
    CLI_BINARY_NAME,
    CLI_CRATE_NAME,
    PRODUCT_NAME,
    Shell,
    Terminal,
    directories,
    system_paths,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use owo_colors::OwoColorize;
use regex::Regex;
use spinners::{
    Spinner,
    Spinners,
};

use super::app::restart_fig;
use crate::util::desktop::desktop_app_running;
use crate::util::is_executable_in_path;
use crate::util::system_info::SupportLevel;

#[derive(Debug, Args, PartialEq, Eq)]
pub struct DoctorArgs {
    /// Run all doctor tests, with no fixes
    #[arg(long, short = 'a')]
    pub all: bool,
    /// Error on warnings
    #[arg(long, short = 's')]
    pub strict: bool,
}

impl DoctorArgs {
    pub async fn execute(self) -> Result<ExitCode> {
        doctor_cli(self.all, self.strict).await
    }
}

enum DoctorFix {
    Sync(Box<dyn FnOnce() -> Result<()> + Send>),
    Async(BoxFuture<'static, Result<()>>),
}

enum DoctorError {
    Warning(Cow<'static, str>),
    Error {
        reason: Cow<'static, str>,
        info: Vec<Cow<'static, str>>,
        fix: Option<DoctorFix>,
        error: Option<eyre::Report>,
    },
}

impl std::fmt::Debug for DoctorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            DoctorError::Warning(msg) => f.debug_struct("Warning").field("msg", msg).finish(),
            DoctorError::Error { reason, info, .. } => f
                .debug_struct("Error")
                .field("reason", reason)
                .field("info", info)
                .finish(),
        }
    }
}

impl std::fmt::Display for DoctorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DoctorError::Warning(warning) => write!(f, "Warning: {warning}"),
            DoctorError::Error { reason, .. } => write!(f, "Error: {reason}"),
        }
    }
}

impl From<eyre::Report> for DoctorError {
    fn from(err: eyre::Report) -> Self {
        DoctorError::Error {
            reason: err.to_string().into(),
            info: vec![],
            fix: None,
            error: Some(err),
        }
    }
}

impl From<fig_util::Error> for DoctorError {
    fn from(err: fig_util::Error) -> Self {
        DoctorError::Error {
            reason: err.to_string().into(),
            info: vec![],
            fix: None,
            error: Some(eyre::Report::from(err)),
        }
    }
}

impl DoctorError {
    fn warning(reason: impl Into<Cow<'static, str>>) -> DoctorError {
        DoctorError::Warning(reason.into())
    }

    fn error(reason: impl Into<Cow<'static, str>>) -> DoctorError {
        DoctorError::Error {
            reason: reason.into(),
            info: vec![],
            fix: None,
            error: None,
        }
    }
}

macro_rules! doctor_warning {
    ($($arg:tt)*) => {{
        DoctorError::warning(format!($($arg)*))
    }}
}

macro_rules! doctor_error {
    ($($arg:tt)*) => {{
        DoctorError::error(format!($($arg)*))
    }}
}

macro_rules! doctor_fix_async {
    ({ reason: $reason:expr,fix: $fix:expr }) => {
        DoctorError::Error {
            reason: $reason.into(),
            info: vec![],
            fix: Some(DoctorFix::Async(Box::pin($fix))),
            error: None,
        }
    };
}

fn check_file_exists(path: impl AsRef<Path>) -> Result<()> {
    if !path.as_ref().exists() {
        eyre::bail!("No file at path {}", path.as_ref().display())
    }
    Ok(())
}

async fn check_socket_perms(path: impl AsRef<Path>) -> Result<(), DoctorError> {
    let path = path.as_ref();
    fig_ipc::validate_socket(&path).await.map_err(|err| DoctorError::Error {
        reason: format!("Failed to validate socket permissions: {err}").into(),
        info: vec![format!("Socket path: {path:?}").into()],
        fix: None,
        error: Some(err.into()),
    })
}

fn command_fix<A, I, D>(args: A, sleep_duration: D) -> Option<DoctorFix>
where
    A: IntoIterator<Item = I> + Send,
    I: AsRef<OsStr> + Send + 'static,
    D: Into<Option<Duration>> + Send + 'static,
{
    let args = args.into_iter().collect::<Vec<_>>();

    Some(DoctorFix::Sync(Box::new(move || {
        if let (Some(exe), Some(remaining)) = (args.first(), args.get(1..))
            && Command::new(exe).args(remaining).status()?.success()
        {
            if let Some(duration) = sleep_duration.into() {
                let spinner = Spinner::new(Spinners::Dots, "Waiting for command to finish...".into());
                std::thread::sleep(duration);
                stop_spinner(Some(spinner)).ok();
            }
            return Ok(());
        }
        eyre::bail!(
            "Failed to run {:?}",
            args.iter()
                .filter_map(|s| s.as_ref().to_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
    })))
}

const CHECKMARK: &str = "✔";
const DOT: &str = "●";
const CROSS: &str = "✘";

fn print_status_result(name: impl Display, status: &Result<(), DoctorError>, verbose: bool) {
    match status {
        Ok(()) => {
            println!("{} {name}", CHECKMARK.green());
        },
        Err(DoctorError::Warning(msg)) => {
            println!("{} {msg}", DOT.yellow());
        },
        Err(DoctorError::Error {
            reason, info, error, ..
        }) => {
            println!("{} {name}: {reason}", CROSS.red());
            if !info.is_empty() {
                println!();
                for infoline in info {
                    println!("  {infoline}");
                }
            }
            if let Some(error) = error
                && verbose
            {
                println!("  {error:?}");
            }
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum DoctorCheckType {
    NormalCheck,
    SoftCheck,
    NoCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(unused)]
enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Platform::MacOs,
            "linux" => Platform::Linux,
            "windows" => Platform::Windows,
            _ => Platform::Other,
        }
    }
}

#[async_trait]
trait DoctorCheck<T = ()>: Sync
where
    T: Sync + Send + Sized,
{
    fn name(&self) -> Cow<'static, str>;

    async fn get_type(&self, _: &T, _platform: Platform) -> DoctorCheckType {
        DoctorCheckType::NormalCheck
    }

    async fn check(&self, context: &T) -> Result<(), DoctorError>;
}

struct FigBinCheck;

#[async_trait]
impl DoctorCheck for FigBinCheck {
    fn name(&self) -> Cow<'static, str> {
        format!("{PRODUCT_NAME} data dir exists").into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        let path = directories::fig_data_dir().map_err(eyre::Report::from)?;
        Ok(check_file_exists(path)?)
    }
}

macro_rules! path_check {
    ($name:ident, $path:expr) => {
        struct $name;

        #[async_trait]
        impl DoctorCheck for $name {
            fn name(&self) -> Cow<'static, str> {
                concat!("PATH contains ~/", $path).into()
            }

            async fn check(&self, _: &()) -> Result<(), DoctorError> {
                match std::env::var("PATH").map(|path| path.contains($path)) {
                    Ok(true) => Ok(()),
                    _ => return Err(doctor_error!(concat!("Path does not contain ~/", $path))),
                }
            }
        }
    };
}

path_check!(LocalBinPathCheck, ".local/bin");

struct AppRunningCheck;

#[async_trait]
impl DoctorCheck for AppRunningCheck {
    fn name(&self) -> Cow<'static, str> {
        format!("{PRODUCT_NAME} is running").into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        if !desktop_app_running() {
            Err(DoctorError::Error {
                reason: format!("{PRODUCT_NAME} is not running").into(),
                info: vec![],
                fix: command_fix(vec![CLI_BINARY_NAME, "launch"], Duration::from_secs(5)),
                error: None,
            })
        } else {
            Ok(())
        }
    }

    async fn get_type(&self, _: &(), platform: Platform) -> DoctorCheckType {
        if platform == Platform::MacOs {
            DoctorCheckType::NormalCheck
        } else {
            DoctorCheckType::NoCheck
        }
    }
}

struct DesktopSocketCheck;

#[async_trait]
impl DoctorCheck for DesktopSocketCheck {
    fn name(&self) -> Cow<'static, str> {
        format!("{PRODUCT_NAME} app socket exists").into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        let fig_socket_path = directories::desktop_socket_path().context("No socket path")?;
        let parent = fig_socket_path.parent().map(PathBuf::from);

        if let Some(parent) = parent
            && !parent.exists()
        {
            return Err(DoctorError::Error {
                reason: format!("{PRODUCT_NAME} socket parent directory does not exist").into(),
                info: vec![format!("Path: {}", fig_socket_path.display()).into()],
                fix: Some(DoctorFix::Sync(Box::new(|| {
                    std::fs::create_dir_all(parent)?;
                    Ok(())
                }))),
                error: None,
            });
        }

        check_file_exists(&fig_socket_path).map_err(|_err| {
            doctor_fix_async!({
                reason: format!("{PRODUCT_NAME} socket missing"),
                fix: restart_fig()
            })
        })?;

        check_socket_perms(fig_socket_path).await
    }

    async fn get_type(&self, _: &(), platform: Platform) -> DoctorCheckType {
        if platform == Platform::MacOs {
            DoctorCheckType::NormalCheck
        } else {
            DoctorCheckType::NoCheck
        }
    }
}

struct SettingsCorruptionCheck;

#[async_trait]
impl DoctorCheck for SettingsCorruptionCheck {
    fn name(&self) -> Cow<'static, str> {
        format!("{PRODUCT_NAME} settings corruption").into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        fig_settings::OldSettings::load().map_err(|_err| DoctorError::Error {
            reason: format!("{PRODUCT_NAME} settings file is corrupted").into(),
            info: vec![],
            fix: Some(DoctorFix::Sync(Box::new(|| {
                std::fs::write(settings_path()?, "{}")?;
                Ok(())
            }))),
            error: None,
        })?;

        Ok(())
    }
}

struct FigIntegrationsCheck;

#[async_trait]
impl DoctorCheck for FigIntegrationsCheck {
    fn name(&self) -> Cow<'static, str> {
        format!("{PRODUCT_NAME} terminal integrations").into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        if let Ok("WarpTerminal") = std::env::var("TERM_PROGRAM").as_deref() {
            return Err(DoctorError::Error {
                reason: "WarpTerminal is not supported".into(),
                info: vec![],
                fix: None,
                error: None,
            });
        }

        if std::env::var_os("__PWSH_LOGIN_CHECKED").is_some() {
            return Err(DoctorError::Error {
                reason: "Powershell is not supported".into(),
                info: vec![],
                fix: None,
                error: None,
            });
        }

        if std::env::var_os("INSIDE_EMACS").is_some() {
            return Err(DoctorError::Error {
                reason: "Emacs is not supported".into(),
                info: vec![],
                fix: None,
                error: None,
            });
        }

        if let Ok("com.vandyke.SecureCRT") = std::env::var("__CFBundleIdentifier").as_deref() {
            return Err(DoctorError::Error {
                reason: "SecureCRT is not supported".into(),
                info: vec![],
                fix: None,
                error: None,
            });
        }

        if std::env::var_os(PROCESS_LAUNCHED_BY_FIG).is_some() {
            return Err(DoctorError::Error {
                reason: format!("{PRODUCT_NAME} can not run in a process it launched").into(),
                info: vec![],
                fix: None,
                error: None,
            });
        }

        if crate::util::system_info::in_ci() {
            return Err(DoctorError::Error {
                reason: "Doctor can not run in CI".into(),
                info: vec![],
                fix: None,
                error: None,
            });
        }

        Ok(())
    }
}

struct DotfileCheck {
    integration: Box<dyn ShellIntegration>,
}

impl DotfileCheck {
    fn short_path(&self) -> String {
        directories::home_dir()
            .ok()
            .and_then(|home_dir| self.integration.path().strip_prefix(&home_dir).ok().map(PathBuf::from))
            .map_or_else(
                || self.integration.path().display().to_string(),
                |path| format!("~/{}", path.display()),
            )
    }
}

#[async_trait]
impl DoctorCheck<Option<Shell>> for DotfileCheck {
    fn name(&self) -> Cow<'static, str> {
        let path = self.short_path();
        let shell = self.integration.get_shell();
        format!("{shell} {path} integration check").into()
    }

    async fn get_type(&self, current_shell: &Option<Shell>, _platform: Platform) -> DoctorCheckType {
        if let Some(shell) = current_shell
            && *shell == self.integration.get_shell()
        {
            return DoctorCheckType::NormalCheck;
        }

        if is_executable_in_path(self.integration.get_shell().to_string()) {
            DoctorCheckType::SoftCheck
        } else {
            DoctorCheckType::NoCheck
        }
    }

    async fn check(&self, _: &Option<Shell>) -> Result<(), DoctorError> {
        let fix_text = format!(
            "Run {} to reinstall shell integrations for {}",
            format!("{CLI_BINARY_NAME} integrations install dotfiles").magenta(),
            self.integration.get_shell()
        );
        match self.integration.is_installed().await {
            Ok(()) => Ok(()),
            Err(InstallationError::NotInstalled(msg) | InstallationError::ImproperInstallation(msg)) => {
                let fix_integration = self.integration.clone();
                Err(DoctorError::Error {
                    reason: msg,
                    info: vec![fix_text.into()],
                    fix: Some(DoctorFix::Async(
                        async move {
                            fix_integration.install().await?;
                            Ok(())
                        }
                        .boxed(),
                    )),
                    error: None,
                })
            },
            Err(err @ InstallationError::FileDoesNotExist(_)) => {
                let fix_integration = self.integration.clone();
                Err(DoctorError::Error {
                    reason: err.to_string().into(),
                    info: vec![fix_text.into()],
                    fix: Some(DoctorFix::Async(
                        async move {
                            fix_integration.install().await?;
                            Ok(())
                        }
                        .boxed(),
                    )),
                    error: Some(eyre::Report::new(err)),
                })
            },
            Err(err @ InstallationError::PermissionDenied { .. }) => {
                let message = err.verbose_message();
                Err(DoctorError::Error {
                    reason: message.title.into(),
                    info: vec![message.message.unwrap().into(), "".into()],
                    fix: None,
                    error: None,
                })
            },
            Err(err) => Err(DoctorError::Error {
                reason: err.to_string().into(),
                info: vec![],
                fix: None,
                error: Some(eyre::Report::new(err)),
            }),
        }
    }
}

pub fn dscl_read(value: impl AsRef<OsStr>) -> Result<String> {
    let username_command = Command::new("id").arg("-un").output().context("Could not get id")?;

    let username: String = String::from_utf8_lossy(&username_command.stdout).trim().into();

    let result = Command::new("dscl")
        .arg(".")
        .arg("-read")
        .arg(format!("/Users/{username}"))
        .arg(value)
        .output()
        .context("Could not read value")?;

    Ok(String::from_utf8_lossy(&result.stdout).trim().into())
}

struct ShellCompatibilityCheck;

#[async_trait]
impl DoctorCheck<DiagnosticsResponse> for ShellCompatibilityCheck {
    fn name(&self) -> Cow<'static, str> {
        "Compatible shell".into()
    }

    async fn check(&self, _: &DiagnosticsResponse) -> Result<(), DoctorError> {
        let shell_regex = Regex::new(r"zsh").unwrap();

        let current_shell = fig_util::get_parent_process_exe();
        let current_shell_valid = current_shell
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .map(|s| {
                let is_match = shell_regex.is_match(&s);
                (s, is_match)
            });

        let default_shell = dscl_read("UserShell");
        let default_shell_valid = default_shell.as_ref().map(|s| (s, shell_regex.is_match(s)));

        match (current_shell_valid, default_shell_valid) {
            (Some((current_shell, false)), _) => {
                return Err(doctor_error!("Current shell {current_shell} incompatible"));
            },
            (_, Ok((default_shell, false))) => {
                return Err(doctor_error!("Default shell {default_shell} incompatible"));
            },
            (None, _) => return Err(doctor_error!("Could not get current shell")),
            (_, Err(_)) => Err(doctor_warning!("Could not get default shell")),
            _ => Ok(()),
        }
    }
}

struct BundlePathCheck;

#[async_trait]
impl DoctorCheck<DiagnosticsResponse> for BundlePathCheck {
    fn name(&self) -> Cow<'static, str> {
        "Correct App Installation Location".into()
    }

    async fn check(&self, diagnostics: &DiagnosticsResponse) -> Result<(), DoctorError> {
        let path = diagnostics.path_to_bundle.clone();
        if path.contains(&format!("{}/{APP_BUNDLE_NAME}", system_paths::APPLICATIONS_DIR)) || path.contains(".toolbox")
        {
            Ok(())
        } else if path.contains(&format!("/Build/Products/Debug/{APP_BUNDLE_NAME}")) {
            Err(DoctorError::Warning(
                format!("Running debug build in {}", path.bold()).into(),
            ))
        } else {
            Err(DoctorError::Error {
                reason: format!("App is installed in {}", path.bold()).into(),
                info: vec![
                    format!("You need to install the app into {}.", system_paths::APPLICATIONS_DIR).into(),
                    "To fix: uninstall and reinstall in the correct location.".into(),
                    "Remember to drag the installed app into the Applications folder.".into(),
                ],
                fix: None,
                error: None,
            })
        }
    }
}

struct AutocompleteEnabledCheck;

#[async_trait]
impl DoctorCheck<DiagnosticsResponse> for AutocompleteEnabledCheck {
    fn name(&self) -> Cow<'static, str> {
        "Autocomplete is enabled".into()
    }

    async fn check(&self, _diagnostics: &DiagnosticsResponse) -> Result<(), DoctorError> {
        if !fig_settings::settings::get_bool_or("autocomplete.disable", false) {
            Ok(())
        } else {
            Err(DoctorError::Error {
                reason: "Autocomplete disabled.".into(),
                info: vec![
                    format!(
                        "To fix run: {}",
                        format!("{CLI_BINARY_NAME} settings autocomplete.disable false").magenta()
                    )
                    .into(),
                ],
                fix: None,
                error: None,
            })
        }
    }
}

macro_rules! dev_mode_check {
    ($struct_name:ident, $check_name:expr, $settings_module:ident, $setting_name:expr) => {
        struct $struct_name;

        #[async_trait]
        impl DoctorCheck for $struct_name {
            fn name(&self) -> Cow<'static, str> {
                $check_name.into()
            }

            async fn check(&self, _: &()) -> Result<(), DoctorError> {
                if let Ok(Some(true)) = fig_settings::$settings_module::get_bool($setting_name) {
                    Err(DoctorError::Warning(concat!($setting_name, " is enabled").into()))
                } else {
                    Ok(())
                }
            }
        }
    };
}

dev_mode_check!(
    AutocompleteDevModeCheck,
    "Autocomplete dev mode",
    settings,
    "autocomplete.developerMode"
);

dev_mode_check!(PluginDevModeCheck, "Plugin dev mode", state, "plugin.developerMode");

struct CliPathCheck;

#[async_trait]
impl DoctorCheck<DiagnosticsResponse> for CliPathCheck {
    fn name(&self) -> Cow<'static, str> {
        "Valid CLI path".into()
    }

    async fn check(&self, _: &DiagnosticsResponse) -> Result<(), DoctorError> {
        let path = std::env::current_exe().context("Could not get executable path.")?;

        let local_bin_path = directories::home_dir()
            .unwrap()
            .join(".local")
            .join("bin")
            .join(CLI_BINARY_NAME);

        if path == local_bin_path
            || path == Path::new(system_paths::USR_LOCAL_BIN).join(CLI_BINARY_NAME)
            || path == Path::new(system_paths::OPT_HOMEBREW_BIN).join(CLI_BINARY_NAME)
        {
            Ok(())
        } else if path.ends_with(Path::new("target/debug").join(CLI_BINARY_NAME))
            || path.ends_with(Path::new("target/release").join(CLI_BINARY_NAME))
            || path.ends_with(format!("target/debug/{CLI_CRATE_NAME}"))
            || path.ends_with(format!("target/release/{CLI_CRATE_NAME}"))
        {
            Err(doctor_warning!(
                "Running debug build in a non-standard location: {}",
                path.display().bold()
            ))
        } else {
            Err(doctor_error!(
                "CLI ({}) must be in {}",
                path.display(),
                local_bin_path.display()
            ))
        }
    }
}

struct AccessibilityCheck;

#[async_trait]
impl DoctorCheck<DiagnosticsResponse> for AccessibilityCheck {
    fn name(&self) -> Cow<'static, str> {
        "Accessibility enabled".into()
    }

    async fn check(&self, diagnostics: &DiagnosticsResponse) -> Result<(), DoctorError> {
        if diagnostics.accessibility != "true" {
            Err(DoctorError::Error {
                reason: "Accessibility is disabled".into(),
                info: vec![],
                fix: command_fix(
                    vec![CLI_BINARY_NAME, "debug", "prompt-accessibility"],
                    Duration::from_secs(1),
                ),
                // fix: Some(DoctorFix::Sync(Box::new(move || {
                //     println!("1. Try enabling accessibility in System Settings");
                //     if !Command::new(CLI_BINARY_NAME)
                //         .args(["debug", "prompt-accessibility"])
                //         .status()?
                //         .success()
                //     {
                //         bail!("Failed to open accessibility in System Settings: {CLI_BINARY_NAME}
                // debug prompt-accessibility");     }

                //     println!("2. Restarting");
                //     println!("3. Reset accessibility");

                //     Ok(())
                // }))),
                error: None,
            })
        } else {
            Ok(())
        }
    }
}

struct DotfilesSymlinkedCheck;

#[async_trait]
impl DoctorCheck<DiagnosticsResponse> for DotfilesSymlinkedCheck {
    fn name(&self) -> Cow<'static, str> {
        "Dotfiles symlinked".into()
    }

    async fn get_type(&self, diagnostics: &DiagnosticsResponse, _platform: Platform) -> DoctorCheckType {
        if diagnostics.symlinked == "true" {
            DoctorCheckType::NormalCheck
        } else {
            DoctorCheckType::NoCheck
        }
    }

    async fn check(&self, _: &DiagnosticsResponse) -> Result<(), DoctorError> {
        Err(DoctorError::Warning(
            "It looks like your dotfiles are symlinked. If you need to make modifications, make sure they're made in \
             the right place."
                .into(),
        ))
    }
}

struct SupportedTerminalCheckContext {
    terminal: Option<Terminal>,
}

struct SupportedTerminalCheck;

#[async_trait]
impl DoctorCheck<SupportedTerminalCheckContext> for SupportedTerminalCheck {
    fn name(&self) -> Cow<'static, str> {
        "Terminal support".into()
    }

    async fn get_type(&self, _: &SupportedTerminalCheckContext, platform: Platform) -> DoctorCheckType {
        if crate::util::system_info::is_remote() {
            DoctorCheckType::NoCheck
        } else {
            match platform {
                Platform::MacOs => DoctorCheckType::NormalCheck,
                // We can promote this to normal check once we have better terminal detection on
                // other platforms, also we should probably use process tree climbing instead of env
                // vars
                _ => DoctorCheckType::SoftCheck,
            }
        }
    }

    async fn check(&self, context: &SupportedTerminalCheckContext) -> Result<(), DoctorError> {
        if context.terminal.is_none() {
            Err(DoctorError::Error {
                reason: "Unsupported terminal".into(),
                info: vec![
                    format!(
                        "__CFBundleIdentifier: {}",
                        std::env::var("__CFBundleIdentifier").unwrap_or_else(|_| "<not-set>".into())
                    )
                    .into(),
                ],
                fix: None,
                error: None,
            })
        } else {
            Ok(())
        }
    }
}

struct SystemVersionCheck;

#[async_trait]
impl DoctorCheck for SystemVersionCheck {
    fn name(&self) -> Cow<'static, str> {
        "OS is supported".into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        let os_version = crate::util::system_info::os_version().wrap_err("Could not get OS Version")?;
        match os_version.support_level() {
            SupportLevel::Supported => Ok(()),
            SupportLevel::SupportedWithCaveat { info } => Err(DoctorError::Warning(info)),
            SupportLevel::InDevelopment { info } => Err(DoctorError::Warning(
                format!(
                    "Support for {os_version} is in development. It may not work properly on your system.\n{}",
                    info.unwrap_or_default()
                )
                .into(),
            )),
            SupportLevel::Unsupported => Err(doctor_error!("{os_version} is not supported")),
        }
    }
}

struct DashboardHostCheck;

#[async_trait]
impl DoctorCheck for DashboardHostCheck {
    fn name(&self) -> Cow<'static, str> {
        "Dashboard is loading from the correct URL".into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        match fig_settings::settings::get_string("developer.dashboard.host")
            .ok()
            .flatten()
        {
            Some(host) => {
                if host.contains("localhost") {
                    Err(DoctorError::Warning(
                        format!("developer.dashboard.host = {host}, delete this setting if Dashboard fails to load")
                            .into(),
                    ))
                } else {
                    Ok(())
                }
            },
            None => Ok(()),
        }
    }
}

struct AutocompleteHostCheck;

#[async_trait]
impl DoctorCheck for AutocompleteHostCheck {
    fn name(&self) -> Cow<'static, str> {
        "Autocomplete is loading from the correct URL".into()
    }

    async fn check(&self, _: &()) -> Result<(), DoctorError> {
        match fig_settings::settings::get_string("developer.autocomplete.host")
            .ok()
            .flatten()
        {
            Some(host) => {
                if host.contains("localhost") {
                    Err(DoctorError::Warning(
                        format!(
                            "developer.autocomplete.host = {host}, delete this setting if Autocomplete fails to load"
                        )
                        .into(),
                    ))
                } else {
                    Ok(())
                }
            },
            None => Ok(()),
        }
    }
}

async fn run_checks_with_context<T, Fut>(
    header: impl AsRef<str>,
    checks: Vec<&dyn DoctorCheck<T>>,
    get_context: impl Fn() -> Fut,
    config: CheckConfiguration,
    spinner: &mut Option<Spinner>,
) -> Result<()>
where
    T: Sync + Send,
    Fut: Future<Output = Result<T>>,
{
    if config.all {
        println!("{}", header.as_ref().dark_grey());
    }
    let mut context = match get_context().await {
        Ok(c) => c,
        Err(e) => {
            println!("Failed to get context: {e:?}");
            eyre::bail!(e);
        },
    };
    for check in checks {
        let name = check.name();
        let check_type: DoctorCheckType = check.get_type(&context, Platform::current()).await;

        if check_type == DoctorCheckType::NoCheck {
            continue;
        }

        let mut result = check.check(&context).await;

        if !config.strict
            && check_type == DoctorCheckType::SoftCheck
            && let Err(DoctorError::Error { reason, .. }) = result
        {
            result = Err(DoctorError::Warning(reason));
        }

        if config.all || result.is_err() {
            stop_spinner(spinner.take())?;
            print_status_result(&name, &result, config.all);
        }

        if config.all {
            continue;
        }

        if let Err(DoctorError::Error { reason, fix, error, .. }) = result {
            if let Some(fixfn) = fix {
                println!("Attempting to fix automatically...");
                if let Err(err) = match fixfn {
                    DoctorFix::Sync(fixfn) => fixfn(),
                    DoctorFix::Async(fixfn) => fixfn.await,
                } {
                    println!("Failed to fix: {err}");
                } else {
                    println!("Re-running check...");
                    println!();
                    if let Ok(new_context) = get_context().await {
                        context = new_context;
                    }
                    let fix_result = check.check(&context).await;
                    print_status_result(&name, &fix_result, config.all);
                    match fix_result {
                        Err(DoctorError::Error { .. }) => {},
                        _ => {
                            continue;
                        },
                    }
                }
            }
            println!();
            match error {
                Some(err) => eyre::bail!(err),
                None => eyre::bail!(reason),
            }
        }
    }

    if config.all {
        println!();
    }

    Ok(())
}

async fn get_shell_context() -> Result<Option<Shell>> {
    Ok(Shell::current_shell())
}

async fn get_terminal_context() -> Result<SupportedTerminalCheckContext> {
    Ok(SupportedTerminalCheckContext {
        terminal: Terminal::parent_terminal(&Context::new()),
    })
}

async fn get_null_context() -> Result<()> {
    Ok(())
}

async fn run_checks(
    header: String,
    checks: Vec<&dyn DoctorCheck>,
    config: CheckConfiguration,
    spinner: &mut Option<Spinner>,
) -> Result<()> {
    run_checks_with_context(header, checks, get_null_context, config, spinner).await
}

fn stop_spinner(spinner: Option<Spinner>) -> Result<()> {
    if let Some(mut sp) = spinner {
        sp.stop();
        execute!(std::io::stdout(), Clear(ClearType::CurrentLine), cursor::Show)?;
        println!();
    }

    Ok(())
}

#[derive(Copy, Clone)]
struct CheckConfiguration {
    all: bool,
    strict: bool,
}

// Doctor
pub async fn doctor_cli(all: bool, strict: bool) -> Result<ExitCode> {
    {
        use nix::unistd::geteuid;
        if geteuid().is_root() {
            eprintln!("{}", "Running doctor as root is not supported.".red().bold());
            if !all {
                eprintln!(
                    "{}",
                    "If you know what you're doing, run the command again with --all.".red()
                );
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    let config = CheckConfiguration { all, strict };

    let mut spinner: Option<Spinner> = None;
    if !config.all {
        spinner = Some(Spinner::new(Spinners::Dots, "Running checks...".into()));
        execute!(std::io::stdout(), cursor::Hide)?;

        ctrlc::set_handler(move || {
            execute!(std::io::stdout(), cursor::Show).ok();
            #[allow(clippy::exit)]
            std::process::exit(1);
        })?;
    }

    let shell_integrations: Vec<_> = Shell::all()
        .iter()
        .map(|shell| shell.get_shell_integrations(&Env::new()))
        .collect::<Result<Vec<_>, fig_integrations::Error>>()?
        .into_iter()
        .flatten()
        .map(|integration| DotfileCheck { integration })
        .collect();

    let mut all_dotfile_checks: Vec<&dyn DoctorCheck<_>> = vec![];
    all_dotfile_checks.extend(shell_integrations.iter().map(|p| p as &dyn DoctorCheck<_>));

    let status: Result<()> = async {
        run_checks_with_context(
            "Let's check your dotfiles...",
            all_dotfile_checks,
            get_shell_context,
            config,
            &mut spinner,
        )
        .await?;

        run_checks(
            format!("Let's make sure {PRODUCT_NAME} is set up correctly..."),
            vec![
                &FigBinCheck,
                &LocalBinPathCheck,
                &SettingsCorruptionCheck,
                &FigIntegrationsCheck,
            ],
            config,
            &mut spinner,
        )
        .await?;

        run_checks(
            "Let's make sure the app is running...".into(),
            vec![&AppRunningCheck, &DesktopSocketCheck],
            config,
            &mut spinner,
        )
        .await?;

        let zsh_hook = ZshHookCheck::probe().await;
        run_checks(
            "Let's see which way zsh gets autocomplete...".into(),
            vec![&zsh_hook],
            config,
            &mut spinner,
        )
        .await?;

        run_checks(
            "Let's see if the app is in a working state...".into(),
            vec![
                &AutocompleteDevModeCheck,
                &PluginDevModeCheck,
                &DashboardHostCheck,
                &AutocompleteHostCheck,
            ],
            config,
            &mut spinner,
        )
        .await?;

        run_checks(
            "Let's check if your system is compatible...".into(),
            vec![&SystemVersionCheck],
            config,
            &mut spinner,
        )
        .await
        .ok();

        {
            run_checks_with_context(
                format!("Let's check {}...", format!("{CLI_BINARY_NAME} diagnostic").bold()),
                vec![
                    &ShellCompatibilityCheck,
                    &BundlePathCheck,
                    &AutocompleteEnabledCheck,
                    &CliPathCheck,
                    &AccessibilityCheck,
                    &DotfilesSymlinkedCheck,
                ],
                super::diagnostics::get_diagnostics,
                config,
                &mut spinner,
            )
            .await?;
        }

        run_checks_with_context(
            "Let's check your terminal integrations...",
            vec![&SupportedTerminalCheck],
            get_terminal_context,
            config,
            &mut spinner,
        )
        .await?;

        Ok(())
    }
    .await;

    let is_error = status.is_err();

    stop_spinner(spinner)?;

    if is_error {
        println!();
        println!("{} Doctor found errors. Please fix them and try again.", CROSS.red());
        println!();
    } else {
        // If early exit is disabled, no errors are thrown
        if !config.all {
            println!("{} Everything looks good!", CHECKMARK.green());
        }
        println!();
    }

    if fig_settings::state::get_bool_or("doctor.prompt-restart-terminal", false) {
        println!(
            "  {}{}",
            "PS. Autocomplete won't work in any existing terminal sessions, ".bold(),
            "only new ones.".bold().italic()
        );
        println!("  (You might want to restart your terminal emulator)");
        fig_settings::state::set_value("doctor.prompt-restart-terminal", false)?;
    }

    Ok(ExitCode::SUCCESS)
}
