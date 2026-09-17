//! Install and uninstall of the components the app manages: shell integrations, the
//! binaries in `~/.local/bin` and the desktop app.
//!
//! Moved here from the former installer crate (which also owned the self-update
//! pipeline) so that the update machinery could be deleted.

use std::path::PathBuf;
use std::sync::Arc;

use fig_os_shim::{
    Context,
    Env,
};
use fig_util::{
    CLI_BINARY_NAME,
    Shell,
    directories,
};

use crate::backup::backup_file;
use crate::error::{
    Error,
    Result,
};
use crate::shell::ShellExt;

bitflags::bitflags! {
    /// The different components that can be installed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct InstallComponents: u64 {
        /// Removal of the integrations from user's dotfiles
        const SHELL_INTEGRATIONS    = 0b00000001;
        /// This handles the removal of the CLI binary, and of the pty binary and its per-shell
        /// copies that older installs left behind
        const BINARY                = 0b00000010;
        const DESKTOP_APP           = 0b00001000;
    }
}

/// What installs from before the shell hook put in `~/.local/bin` for the pty binary (`figterm`): a
/// link to it and a copy per shell. Nothing creates them any more; uninstalling still removes them.
const LEGACY_PTY_BINARIES: [&str; 3] = ["figterm", "zsh (figterm)", "bash (figterm)"];

/// Removes the integration blocks from the user's rc files.
///
/// Each rc file is copied into the timestamped backup directory first: `uninstall` rewrites the
/// file whether or not anything was found to remove (it also trims trailing whitespace and
/// normalises the final newline), so it is destructive even in the no-op case.
async fn uninstall_shell_integrations(env: &Env) -> Result<()> {
    let backup_dir = directories::utc_backup_dir()?;
    for shell in [Shell::Bash, Shell::Zsh] {
        for integration in shell.get_shell_integrations(env)? {
            let path = integration.path();
            backup_file(&path, Some(&backup_dir))?;
            integration.uninstall().await?;
        }
    }
    Ok(())
}

pub async fn uninstall(components: InstallComponents, ctx: Arc<Context>) -> Result<()> {
    // This used to run unconditionally, so `q _ uninstall --binary` stripped the shell
    // integrations out of the user's rc files as a side effect — and `internal/mod.rs` calls
    // `uninstall` twice (once for BINARY, once for the rest), so it happened twice per run.
    if components.contains(InstallComponents::SHELL_INTEGRATIONS) {
        uninstall_shell_integrations(ctx.env()).await?;
    }

    if components.contains(InstallComponents::BINARY) {
        let remove_binary = |path: PathBuf| async move {
            match tokio::fs::remove_file(&path).await {
                Ok(_) => tracing::info!("Removed binary: {path:?}"),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
                Err(err) => tracing::warn!(%err, "Failed to remove binary: {path:?}"),
            }
        };

        let folders = [directories::home_local_bin()?];

        for folder in folders {
            remove_binary(folder.join(CLI_BINARY_NAME)).await;
            for name in LEGACY_PTY_BINARIES {
                remove_binary(folder.join(name)).await;
            }
        }
    }

    if components.contains(InstallComponents::DESKTOP_APP) {
        uninstall_desktop().await?;
        // Must be last -- this will kill the running desktop process if this is
        // called from the desktop app.
        let quit_res = tokio::process::Command::new("killall")
            .args([fig_util::consts::APP_PROCESS_NAME])
            .output()
            .await;
        if let Err(err) = quit_res {
            tracing::warn!("Failed to quit running desktop app: {err}");
        }
    }

    Ok(())
}

pub async fn install(components: InstallComponents, env: &Env) -> Result<()> {
    if components.contains(InstallComponents::SHELL_INTEGRATIONS) {
        let mut errs: Vec<Error> = vec![];
        for shell in Shell::all() {
            match shell.get_shell_integrations(env) {
                Ok(integrations) => {
                    for integration in integrations {
                        if let Err(e) = integration.install().await {
                            errs.push(e);
                        }
                    }
                },
                Err(e) => {
                    errs.push(e);
                },
            }
        }

        if let Some(err) = errs.pop() {
            return Err(err);
        }
    }

    Ok(())
}

/// Removes everything the desktop app left on the machine: the launch agent, the defaults
/// domain, the data directory and the app bundle itself.
async fn uninstall_desktop() -> Result<()> {
    use fig_util::consts::{
        APP_BUNDLE_ID,
        LAUNCH_AGENT_PREFIX,
    };
    use tokio::fs;
    use tracing::warn;

    // Remove launch agents
    if let Ok(home) = directories::home_dir() {
        let launch_agents = home.join("Library").join("LaunchAgents");
        remove_in_dir_with_prefix_unless(&launch_agents, LAUNCH_AGENT_PREFIX, |p| p.contains("daemon")).await;
    } else {
        warn!("Could not find home directory");
    }

    // Delete defaults
    tokio::process::Command::new("defaults")
        .args(["delete", APP_BUNDLE_ID])
        .output()
        .await
        .map_err(|err| warn!("Failed to delete defaults: {err}"))
        .ok();

    // Delete data dir
    if let Ok(fig_data_dir) = directories::fig_data_dir() {
        for file in std::fs::read_dir(fig_data_dir).ok().into_iter().flatten().flatten() {
            if let Ok(metadata) = file.metadata() {
                if metadata.is_dir() {
                    fs::remove_dir_all(file.path())
                        .await
                        .map_err(|err| warn!("Failed to remove data dir: {err}"))
                        .ok();
                } else {
                    fs::remove_file(file.path())
                        .await
                        .map_err(|err| warn!("Failed to remove data dir: {err}"))
                        .ok();
                }
            }
        }
    }

    let app_path = fig_util::app_bundle_path();
    if app_path.exists() {
        fs::remove_dir_all(&app_path)
            .await
            .map_err(|err| warn!("Failed to remove {app_path:?}: {err}"))
            .ok();
    }

    Ok(())
}

async fn remove_in_dir_with_prefix_unless(dir: &std::path::Path, prefix: &str, unless: impl Fn(&str) -> bool) {
    use tokio::fs;

    if let Ok(mut entries) = fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with(prefix)
                && !unless(name)
            {
                fs::remove_file(entry.path()).await.ok();
                fs::remove_dir_all(entry.path()).await.ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use fig_os_shim::ContextBuilder;

    use super::*;

    /// `uninstall` used to strip the integration blocks out of the user's rc files no matter which
    /// components were asked for, so `q _ uninstall --binary` rewrote `.zshrc`/`.bashrc` as a side
    /// effect — twice, because `internal/mod.rs` calls `uninstall` once for `BINARY` and once for
    /// the remaining components.
    ///
    /// `InstallComponents::empty()` is used deliberately: it is the only combination that touches
    /// nothing outside the fake `$HOME`, since `BINARY` removes real files from `~/.local/bin` and
    /// the shell integrations delete real files from the data directory.
    #[tokio::test]
    async fn uninstall_leaves_rc_files_alone_without_the_shell_integrations_flag() {
        let home = tempfile::tempdir().unwrap();
        let rc_files = [".zshrc", ".zprofile", ".bashrc", ".bash_profile", ".profile"];
        let contents = "# my shell config\nexport PATH=\"$HOME/bin:$PATH\"\n\n\n";
        for name in rc_files {
            std::fs::write(home.path().join(name), contents).unwrap();
        }

        let ctx = ContextBuilder::new()
            .with_env_var("HOME", home.path().to_str().unwrap())
            .build();

        uninstall(InstallComponents::empty(), ctx).await.unwrap();

        for name in rc_files {
            assert_eq!(
                std::fs::read_to_string(home.path().join(name)).unwrap(),
                contents,
                "{name} was modified by an uninstall that did not ask for SHELL_INTEGRATIONS"
            );
        }
    }
}
