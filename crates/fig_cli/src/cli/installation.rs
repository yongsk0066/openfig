//! Installation, uninstallation, and update of the CLI.

use std::process::ExitCode;

use anstream::{
    eprintln,
    println,
};
use crossterm::style::Stylize;
use eyre::{
    Result,
    bail,
};
use fig_integrations::install::{
    InstallComponents,
    install,
};
use fig_os_shim::Env;
use fig_util::CLI_BINARY_NAME;
use tracing::warn;

use crate::util::choose;

pub async fn install_cli(
    install_components: InstallComponents,
    no_confirm: bool,
    force: bool,
    global: bool,
) -> Result<ExitCode> {
    let env = Env::new();

    {
        use nix::unistd::geteuid;
        if geteuid().is_root() && !global {
            eprintln!("{}", "Installing as root is not supported.".red().bold());
            if !force {
                eprintln!(
                    "{}",
                    "If you know what you're doing, run the command again with --force.".red()
                );
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    if install_components.contains(InstallComponents::SHELL_INTEGRATIONS) && !global {
        let mut manual_install = if no_confirm {
            false
        } else {
            if !dialoguer::console::user_attended() {
                eyre::bail!("You must run with --no-confirm if unattended");
            }

            match choose(
                format!(
                    "Do you want {CLI_BINARY_NAME} to modify your shell config (you will have to manually do this otherwise)?",
                ),
                &["Yes", "No"],
            )? {
                Some(1) => true,
                Some(_) => false,
                None => bail!("No option selected"),
            }
        };
        if !manual_install && let Err(err) = install(InstallComponents::SHELL_INTEGRATIONS, &env).await {
            println!("{}", "Could not automatically install:".bold());
            println!("{err}");
            manual_install = true;
        }
        if !no_confirm && manual_install {
            let shell_dir = fig_util::directories::fig_data_dir_utf8()?.join("shell");
            let shell_dir = shell_dir
                .strip_prefix(fig_util::directories::home_dir()?)
                .unwrap_or(&shell_dir);

            println!();
            println!("To install the integrations manually, you will have to add the following to your rc files");
            println!("This step is required for the application to function properly");
            println!();
            println!("At the top of your .zshrc file:");
            println!(". \"$HOME/{shell_dir}/zshrc.pre.zsh\"");
            println!();
            println!("At the bottom of your .zshrc file:");
            println!(". \"$HOME/{shell_dir}/zshrc.post.zsh\"");
            println!();

            if let Err(err) = install(InstallComponents::SHELL_INTEGRATIONS, &env).await {
                println!("Could not install required files:");
                println!("{err}");
            }
        }
    }

    if install_components.contains(InstallComponents::SHELL_INTEGRATIONS) && global {
        // TODO: fix this, we do not error out at the moment as the fix is in `install.sh` which works
        // for the CloudShell team
        warn!("Global install is not supported for shell integrations");
    }

    if !global {
        // The dashboard's onboarding modal reappears on every launch until
        // `desktop.completedOnboarding` is set, and only the modal's last step used to set it
        // (`fig_desktop/src/request/onboarding.rs`). Someone who set the shell integrations up
        // from the CLI instead got the "Welcome!" modal in front of the dashboard forever.
        //
        // `state` is the local sqlite db, not an IPC call, so this works with the desktop app
        // stopped — which is the whole point of setting it here.
        if let Err(err) = fig_settings::state::set_value("desktop.completedOnboarding", true) {
            warn!(%err, "Failed to record that onboarding is complete");
        }
    }

    Ok(ExitCode::SUCCESS)
}
