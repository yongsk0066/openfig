pub mod local_state;

use std::io::{
    Write,
    stdout,
};
use std::process::ExitCode;

use clap::{
    Args,
    Subcommand,
};
use eyre::Result;
use fig_integrations::install::InstallComponents;
use fig_util::directories;
use sysinfo::System;

use crate::cli::installation::install_cli;

#[derive(Debug, Args, PartialEq, Eq)]
pub struct InstallArgs {
    /// Install only the shell integrations
    #[arg(long)]
    pub dotfiles: bool,
    /// Don't confirm automatic installation.
    #[arg(long)]
    pub no_confirm: bool,
    /// Force installation
    #[arg(long)]
    pub force: bool,
    /// Install globally
    #[arg(long)]
    pub global: bool,
}

impl From<InstallArgs> for InstallComponents {
    fn from(args: InstallArgs) -> Self {
        let InstallArgs { dotfiles, .. } = args;
        if dotfiles {
            let mut install_components = InstallComponents::empty();
            install_components.set(InstallComponents::SHELL_INTEGRATIONS, dotfiles);
            install_components
        } else {
            InstallComponents::all()
        }
    }
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
#[command(hide = true, alias = "_")]
pub enum InternalSubcommand {
    /// Change the local-state file
    LocalState(local_state::LocalStateArgs),
    /// Install the cli
    Install(InstallArgs),
    Hostname,
    SocketsDir,
    Uuidgen,
}

impl InternalSubcommand {
    pub async fn execute(self) -> Result<ExitCode> {
        match self {
            InternalSubcommand::Install(args) => {
                let no_confirm = args.no_confirm;
                let force = args.force;
                let global = args.global;
                install_cli(args.into(), no_confirm, force, global).await
            },
            InternalSubcommand::LocalState(local_state) => {
                local_state.execute().await?;
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::Hostname => {
                if let Ok(hostname) = System::host_name()
                    && write!(stdout(), "{hostname}").is_ok()
                {
                    return Ok(ExitCode::SUCCESS);
                }
                Ok(ExitCode::FAILURE)
            },
            InternalSubcommand::SocketsDir => {
                writeln!(stdout(), "{}", directories::sockets_dir_utf8()?).ok();
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::Uuidgen => {
                let _ = writeln!(stdout(), "{}", uuid::Uuid::new_v4());
                Ok(ExitCode::SUCCESS)
            },
        }
    }
}
