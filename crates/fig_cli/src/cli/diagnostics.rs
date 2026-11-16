use std::io::{
    IsTerminal,
    stdout,
};
use std::process::ExitCode;

use anstream::println;
use clap::Args;
use color_eyre::Result;
use crossterm::terminal::{
    Clear,
    ClearType,
};
use crossterm::{
    cursor,
    execute,
};
use eyre::ContextCompat;
use fig_ipc::local::send_recv_command_to_socket;
use fig_proto::local::command::Command;
use fig_proto::local::command_response::Response;
use fig_proto::local::{
    DiagnosticsCommand,
    DiagnosticsResponse,
};
use spinners::{
    Spinner,
    Spinners,
};

use super::OutputFormat;

#[derive(Debug, Args, PartialEq, Eq)]
pub struct DiagnosticArgs {
    /// The format of the output
    #[arg(long, short, value_enum, default_value_t)]
    format: OutputFormat,
    /// Force limited diagnostic output
    #[arg(long)]
    force: bool,
}

impl DiagnosticArgs {
    pub async fn execute(&self) -> Result<ExitCode> {
        if !self.force && !crate::util::desktop::desktop_app_running() {
            use fig_util::{
                CLI_BINARY_NAME,
                PRODUCT_NAME,
            };
            use owo_colors::OwoColorize;

            println!(
                "\n→ {PRODUCT_NAME} app is not running.\n  Please launch {PRODUCT_NAME} app with {} or run {} to get limited diagnostics.",
                format!("{CLI_BINARY_NAME} launch").magenta(),
                format!("{CLI_BINARY_NAME} diagnostic --force").magenta()
            );
            return Ok(ExitCode::FAILURE);
        }

        let spinner = if stdout().is_terminal() {
            Some(Spinner::new(Spinners::Dots, "Generating...".into()))
        } else {
            None
        };

        if spinner.is_some() {
            execute!(std::io::stdout(), cursor::Hide)?;

            ctrlc::set_handler(move || {
                execute!(std::io::stdout(), cursor::Show).ok();
                std::process::exit(1);
            })?;
        }

        let diagnostics = Diagnostics::new().await;

        if let Some(mut sp) = spinner {
            sp.stop();
            execute!(std::io::stdout(), Clear(ClearType::CurrentLine), cursor::Show)?;
            println!();
        }

        self.format.print(
            || diagnostics.user_readable().expect("Failed to run user_readable()"),
            || &diagnostics,
        );

        Ok(ExitCode::SUCCESS)
    }
}

pub async fn get_diagnostics() -> Result<DiagnosticsResponse> {
    let response = send_recv_command_to_socket(Command::Diagnostics(DiagnosticsCommand {}))
        .await?
        .context("Received EOF while reading diagnostics")?;

    match response.response {
        Some(Response::Diagnostics(diagnostics)) => Ok(diagnostics),
        _ => eyre::bail!("Invalid response"),
    }
}

// ---------------------------------------------------------------------------------------------
// Absorbed from the former `fig_diagnostic` crate: 261 lines whose only consumer was this file.
// ---------------------------------------------------------------------------------------------

use std::collections::BTreeMap;

use fig_os_shim::Context;
use fig_util::consts::build::HASH;
use fig_util::{
    Shell,
    Terminal,
};
use serde::Serialize;
use sysinfo::{
    CpuRefreshKind,
    MemoryRefreshKind,
    RefreshKind,
    System,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::util::system_info::{
    OSVersion,
    os_version,
};

fn serialize_display_option<D, S>(display: &Option<D>, serializer: S) -> Result<S::Ok, S::Error>
where
    D: std::fmt::Display,
    S: serde::Serializer,
{
    match display {
        Some(display) => serializer.serialize_str(&display.to_string()),
        None => serializer.serialize_none(),
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// The only field the deleted `fig_util::manifest` module ever produced for anyone: 231 lines of
/// `Manifest`/`ManagedBy`/`TargetTriple`/`Channel` deserialisation whose one caller read this
/// single hard-coded constant (`manifest()` never read a file — it returned a literal).
const BUILD_VARIANT: &str = "full";

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct BuildDetails {
    version: String,
    hash: Option<&'static str>,
    date: Option<String>,
    variant: String,
}

impl BuildDetails {
    fn new() -> BuildDetails {
        let date = fig_util::consts::build::DATETIME
            .and_then(|input| OffsetDateTime::parse(input, &Rfc3339).ok())
            .and_then(|time| {
                let rfc3339 = time.format(&Rfc3339).ok()?;
                let duration = OffsetDateTime::now_utc() - time;
                Some(format!("{rfc3339} ({duration:.0} ago)"))
            });

        BuildDetails {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            hash: HASH,
            date,
            variant: BUILD_VARIANT.to_owned(),
        }
    }
}

fn serialize_os_version<S>(version: &Option<OSVersion>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match version {
        Some(version) => serializer.serialize_str(&version.to_string()),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SystemInfo {
    #[serde(serialize_with = "serialize_os_version")]
    os: Option<OSVersion>,
    chip: Option<String>,
    total_cores: Option<usize>,
    memory: Option<String>,
}

impl SystemInfo {
    fn new() -> SystemInfo {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );

        let mut hardware_info = SystemInfo {
            os: os_version().cloned(),
            chip: None,
            total_cores: System::physical_core_count().ok(),
            memory: system
                .as_ref()
                .ok()
                .map(|system| format!("{:0.2} GB", system.total_memory() as f32 / 2.0_f32.powi(30))),
        };

        if let Some(processor) = system.as_ref().ok().and_then(|system| system.cpus().first()) {
            hardware_info.chip = Some(processor.brand().into());
        }

        hardware_info
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EnvVarDiagnostic {
    env_vars: BTreeMap<String, String>,
}

impl EnvVarDiagnostic {
    fn new() -> EnvVarDiagnostic {
        let env_vars = std::env::vars()
            .filter(|(key, _)| {
                let fig_var = fig_util::env_var::ALL.contains(&key.as_str());
                let other_var = [
                    // General env vars
                    "SHELL",
                    "DISPLAY",
                    "PATH",
                    "TERM",
                    "ZDOTDIR",
                    // Linux vars
                    "XDG_CURRENT_DESKTOP",
                    "XDG_SESSION_DESKTOP",
                    "XDG_SESSION_TYPE",
                    "GLFW_IM_MODULE",
                    "GTK_IM_MODULE",
                    "QT_IM_MODULE",
                    "XMODIFIERS",
                    // Macos vars
                    "__CFBundleIdentifier",
                ]
                .contains(&key.as_str());

                fig_var || other_var
            })
            .map(|(key, value)| {
                // sanitize username from values
                let username = format!("/{}", whoami::username().unwrap_or_else(|_| "unknown".into()));
                (key, value.replace(&username, "/USER"))
            })
            .collect();

        EnvVarDiagnostic { env_vars }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CurrentEnvironment {
    cwd: Option<String>,
    cli_path: Option<String>,
    os: &'static str,
    shell_path: Option<String>,
    shell_version: Option<String>,
    #[serde(serialize_with = "serialize_display_option")]
    terminal: Option<Terminal>,
    #[serde(skip_serializing_if = "is_false")]
    in_ssh: bool,
    #[serde(skip_serializing_if = "is_false")]
    in_ci: bool,
    #[serde(skip_serializing_if = "is_false")]
    in_codespaces: bool,
}

impl CurrentEnvironment {
    async fn new() -> CurrentEnvironment {
        let ctx = Context::new();

        let username = format!("/{}", whoami::username().unwrap_or_else(|_| "unknown".into()));

        let shell_path = fig_os_shim::process_info::get_parent_process_exe(&ctx)
            .map(|p| p.to_string_lossy().replace(&username, "/USER"));
        let shell_version = Shell::current_shell_version().await.map(|(_, v)| v).ok();

        let cwd = ctx
            .env()
            .current_dir()
            .ok()
            .map(|path| path.to_string_lossy().replace(&username, "/USER"));

        // The path of *this* binary. This was a copy-paste of the `cwd` expression above
        // (`current_dir()`), so `q diagnostic` reported the working directory twice and never
        // told you which `q` you were actually running — the single most useful line in the
        // report when there is more than one install on the machine.
        let cli_path = std::env::current_exe()
            .ok()
            .map(|path| path.to_string_lossy().replace(&username, "/USER"));

        // The only platform this fork builds for; the report has always spelled it `Mac`.
        let os = "Mac";
        let terminal = Terminal::parent_terminal(&ctx);

        let in_ssh = crate::util::system_info::in_ssh();
        let in_ci = crate::util::system_info::in_ci();
        let in_codespaces = crate::util::system_info::in_codespaces();

        CurrentEnvironment {
            shell_path,
            shell_version,
            cwd,
            cli_path,
            os,
            terminal,
            in_ssh,
            in_ci,
            in_codespaces,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Diagnostics {
    build_details: BuildDetails,
    system_info: SystemInfo,
    environment: CurrentEnvironment,
    #[serde(flatten)]
    environment_variables: EnvVarDiagnostic,
}

impl Diagnostics {
    pub async fn new() -> Diagnostics {
        Diagnostics {
            build_details: BuildDetails::new(),
            system_info: SystemInfo::new(),
            environment: CurrentEnvironment::new().await,
            environment_variables: EnvVarDiagnostic::new(),
        }
    }

    pub fn user_readable(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_diagnostics_user_readable() {
        let diagnostics = Diagnostics::new().await;
        let toml = diagnostics.user_readable().unwrap();
        assert!(!toml.is_empty());
    }
}
