//! CLI functionality

pub mod app;
mod completion;
mod debug;
mod diagnostics;
mod doctor;
mod hook;
mod init;
mod installation;
mod integrations;
pub mod internal;
mod settings;
mod theme;
mod uninstall;

use std::io::{
    Write as _,
    stdout,
};
use std::process::ExitCode;
use std::sync::LazyLock;

use anstream::{
    eprintln,
    println,
};
use clap::{
    ArgAction,
    CommandFactory,
    Parser,
    Subcommand,
    ValueEnum,
};
use crossterm::style::Stylize;
use eyre::{
    Result,
    WrapErr,
};
use fig_ipc::local::open_ui_element;
use fig_log::{
    LogArgs,
    initialize_logging,
};
use fig_proto::local::UiElement;
use fig_util::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
    directories,
};
use serde::Serialize;
use tracing::level_filters::LevelFilter;
use tracing::{
    Level,
    debug,
};

use self::integrations::IntegrationsSubcommands;
use crate::util::CliContext;
use crate::util::desktop::{
    LaunchArgs,
    launch_fig_desktop,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Outputs the results as markdown
    #[default]
    Plain,
    /// Outputs the results as JSON
    Json,
    /// Outputs the results as pretty print JSON
    JsonPretty,
}

impl OutputFormat {
    pub fn print<T, TFn, J, JFn>(&self, text_fn: TFn, json_fn: JFn)
    where
        T: std::fmt::Display,
        TFn: FnOnce() -> T,
        J: Serialize,
        JFn: FnOnce() -> J,
    {
        match self {
            OutputFormat::Plain => println!("{}", text_fn()),
            OutputFormat::Json => println!("{}", serde_json::to_string(&json_fn()).unwrap()),
            OutputFormat::JsonPretty => println!("{}", serde_json::to_string_pretty(&json_fn()).unwrap()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Processes {
    /// Desktop Process
    App,
}

/// The CLI subcommands
#[deny(missing_docs)]
#[derive(Debug, PartialEq, Subcommand)]
pub enum CliRootCommands {
    /// Hook commands
    #[command(subcommand, hide = true)]
    Hook(hook::HookSubcommand),
    /// Debug the app
    #[command(subcommand)]
    Debug(debug::DebugSubcommand),
    /// Customize appearance & behavior
    #[command(alias("setting"))]
    Settings(settings::SettingsArgs),
    /// Setup cli components
    #[command(alias("install"))]
    Setup(internal::InstallArgs),
    /// Uninstall the desktop app and its integrations
    #[command(hide = true)]
    Uninstall {
        /// Force uninstall
        #[arg(long, short = 'y')]
        no_confirm: bool,
    },
    /// Run diagnostic tests
    #[command(alias("diagnostics"))]
    Diagnostic(diagnostics::DiagnosticArgs),
    /// Generate the dotfiles for the given shell
    Init(init::InitArgs),
    /// Get or set theme
    Theme(theme::ThemeArgs),
    /// Fix and diagnose common issues
    Doctor(doctor::DoctorArgs),
    /// Generate CLI completion spec
    #[command(hide = true)]
    Completion(completion::CompletionArgs),
    /// Internal subcommands
    #[command(subcommand, hide = true)]
    Internal(internal::InternalSubcommand),
    /// Launch the desktop app
    Launch,
    /// Quit the desktop app
    Quit,
    /// Restart the desktop app
    Restart {
        /// The process to restart
        #[arg(value_enum, default_value_t = Processes::App, hide = true)]
        process: Processes,
    },
    /// Manage system integrations
    #[command(subcommand, alias("integration"))]
    Integrations(IntegrationsSubcommands),
    /// Version
    #[command(hide = true)]
    Version,
    /// Open the dashboard
    Dashboard,
}

/// The root help screen.
///
/// `PRODUCT_NAME` and `CLI_BINARY_NAME` come from `branding.env` and are not string
/// literals, so this is built on first use rather than being a `cstr!` constant. The box
/// is drawn to a fixed width and the two interpolated names only ever appear at the end of
/// a line, so a longer binary name cannot misalign it.
static HELP_TEXT: LazyLock<String> = LazyLock::new(|| {
    color_print::cformat!(
        "\n\
<magenta,em>{cli}</magenta,em> ({product} CLI)\n\
\n\
<magenta,em>Popular Subcommands</magenta,em>              <black!><em>Usage:</em> {cli} [subcommand]</black!>\n\
╭──────────────────────────────────────────────╮\n\
│ <em>doctor</em>       <black!>Debug installation issues</black!>       │\n\
│ <em>settings</em>     <black!>Customize appearance & behavior</black!> │\n\
│ <em>integrations</em> <black!>Manage system integrations</black!>      │\n\
│ <em>theme</em>        <black!>Get or set the theme</black!>            │\n\
│ <em>quit</em>         <black!>Quit the app</black!>                    │\n\
╰──────────────────────────────────────────────╯\n\
\n\
<black!>To see all subcommands, use:</black!>\n\
\x20<black!>❯</black!> {cli} --help-all\n\
ㅤ\n",
        cli = CLI_BINARY_NAME,
        product = PRODUCT_NAME,
    )
});

#[derive(Debug, Parser, PartialEq, Default)]
#[command(version, about, name = CLI_BINARY_NAME, help_template = HELP_TEXT.as_str())]
pub struct Cli {
    #[command(subcommand)]
    pub subcommand: Option<CliRootCommands>,
    /// Increase logging verbosity
    #[arg(long, short = 'v', action = ArgAction::Count, global = true)]
    pub verbose: u8,
    /// Print help for all subcommands
    #[arg(long)]
    help_all: bool,
}

impl Cli {
    pub async fn execute(self) -> Result<ExitCode> {
        // Resolve the level once and use it for both decisions below. `log_file_path` used to be
        // computed from `fig_log::get_log_level_max()`, which `initialize_logging` only updates
        // *after* it has read this struct — so `-vvv` raised the level but never created
        // `cli.log`, which is the one thing `-vvv` is for.
        let verbosity = match self.verbose {
            0 => None,
            1 => Some(Level::WARN),
            2 => Some(Level::INFO),
            3 => Some(Level::DEBUG),
            _ => Some(Level::TRACE),
        };
        let max_level = verbosity.map_or_else(fig_log::get_log_level_max, LevelFilter::from);

        // `q init`, `q completion` and the `q _ …` internals write to stdout expecting the shell
        // to `eval` it or capture it in `$(…)`. A log line in that stream is a syntax error in the
        // user's shell startup, so the stdout logger is off for them whatever `Q_LOG_STDOUT` or
        // `-v` say; the file log still gets everything.
        let stdout_is_machine_readable = matches!(
            self.subcommand,
            Some(CliRootCommands::Init(_) | CliRootCommands::Completion(_) | CliRootCommands::Internal(_))
        );

        // Initialize our logger and keep around the guard so logging can perform as expected.
        let _log_guard = initialize_logging(LogArgs {
            log_level: verbosity.map(|level| level.to_string()),
            log_to_stdout: !stdout_is_machine_readable && (fig_os_shim::Env::new().q_log_stdout() || self.verbose > 0),
            log_file_path: (max_level >= Level::DEBUG)
                .then(|| directories::logs_dir().map(|dir| dir.join("cli.log")))
                .and_then(Result::ok),
            delete_old_log_file: false,
        });

        debug!(command =? std::env::args().collect::<Vec<_>>(), "Command ran");

        if self.help_all {
            return self.print_help_all();
        }

        let cli_context = CliContext::new();

        match self.subcommand {
            Some(subcommand) => match subcommand {
                CliRootCommands::Setup(args) => {
                    let no_confirm = args.no_confirm;
                    let force = args.force;
                    let global = args.global;
                    installation::install_cli(args.into(), no_confirm, force, global).await
                },
                CliRootCommands::Uninstall { no_confirm } => uninstall::uninstall_command(no_confirm).await,
                CliRootCommands::Diagnostic(args) => args.execute().await,
                CliRootCommands::Init(args) => args.execute().await,
                CliRootCommands::Doctor(args) => args.execute().await,
                CliRootCommands::Hook(hook_subcommand) => hook_subcommand.execute().await,
                CliRootCommands::Theme(theme_args) => theme_args.execute().await,
                CliRootCommands::Settings(settings_args) => settings_args.execute(&cli_context).await,
                CliRootCommands::Debug(debug_subcommand) => debug_subcommand.execute().await,
                CliRootCommands::Completion(args) => args.execute(),
                CliRootCommands::Internal(internal_subcommand) => internal_subcommand.execute().await,
                CliRootCommands::Launch => launch_dashboard().await,
                CliRootCommands::Quit => crate::util::quit_fig(true).await,
                CliRootCommands::Restart { .. } => {
                    app::restart_fig().await?;
                    launch_dashboard().await
                },
                CliRootCommands::Integrations(subcommand) => subcommand.execute().await,
                CliRootCommands::Version => Self::print_version(),
                CliRootCommands::Dashboard => launch_dashboard().await,
            },
            // Root command
            None => {
                Self::command().print_help()?;
                Ok(ExitCode::SUCCESS)
            },
        }
    }

    #[allow(clippy::unused_self)]
    fn print_help_all(&self) -> Result<ExitCode> {
        let mut cmd = Self::command().help_template("{all-args}");
        eprintln!();
        eprintln!(
            "{}\n    {CLI_BINARY_NAME} [OPTIONS] [SUBCOMMAND]\n",
            "USAGE:".bold().underlined(),
        );
        cmd.print_long_help()?;
        Ok(ExitCode::SUCCESS)
    }

    fn print_version() -> Result<ExitCode> {
        let _ = writeln!(stdout(), "{}", Self::command().render_version());
        Ok(ExitCode::SUCCESS)
    }
}

async fn launch_dashboard() -> Result<ExitCode> {
    launch_fig_desktop(LaunchArgs {
        wait_for_socket: true,
        open_dashboard: true,
        verbose: true,
    })?;

    println!("Opening {PRODUCT_NAME} dashboard");

    open_ui_element(UiElement::MissionControl, None)
        .await
        .context("Failed to open dashboard")?;

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn debug_assert() {
        Cli::command().debug_assert();
    }

    macro_rules! assert_parse {
        (
            [ $($args:expr),+ ],
            $subcommand:expr
        ) => {
            assert_eq!(
                Cli::parse_from([CLI_BINARY_NAME, $($args),*]),
                Cli {
                    subcommand: Some($subcommand),
                    ..Default::default()
                }
            );
        };
    }

    /// Test flag parsing for the top level [Cli]
    #[test]
    fn test_flags() {
        assert_eq!(Cli::parse_from([CLI_BINARY_NAME, "-v"]), Cli {
            subcommand: None,
            verbose: 1,
            help_all: false,
        });

        assert_eq!(Cli::parse_from([CLI_BINARY_NAME, "-vvv"]), Cli {
            subcommand: None,
            verbose: 3,
            help_all: false,
        });

        assert_eq!(Cli::parse_from([CLI_BINARY_NAME, "--help-all"]), Cli {
            subcommand: None,
            verbose: 0,
            help_all: true,
        });
    }

    /// This test validates that the restart command maintains the same CLI facing definition
    ///
    /// If this changes, you must also change how it is called from other locations
    #[test]
    fn test_restart() {
        assert_parse!(["restart", "app"], CliRootCommands::Restart {
            process: Processes::App
        });
    }

    #[test]
    fn test_doctor() {
        assert_parse!(
            ["doctor"],
            CliRootCommands::Doctor(doctor::DoctorArgs {
                all: false,
                strict: false,
            })
        );
        assert_parse!(
            ["doctor", "--all"],
            CliRootCommands::Doctor(doctor::DoctorArgs {
                all: true,
                strict: false,
            })
        );
        assert_parse!(
            ["doctor", "--strict"],
            CliRootCommands::Doctor(doctor::DoctorArgs {
                all: false,
                strict: true,
            })
        );
        assert_parse!(
            ["doctor", "-a", "-s"],
            CliRootCommands::Doctor(doctor::DoctorArgs {
                all: true,
                strict: true,
            })
        );
    }
}
