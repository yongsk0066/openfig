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
    CHAT_BINARY_NAME,
    CLI_BINARY_NAME,
    OLD_CLI_BINARY_NAMES,
    OLD_PTY_BINARY_NAMES,
    PTY_BINARY_NAME,
    Shell,
    directories,
};

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
        /// This handles the removal of the CLI and pty binaries as well as legacy copies
        const BINARY                = 0b00000010;
        const DESKTOP_APP           = 0b00001000;
    }
}

pub async fn uninstall(components: InstallComponents, ctx: Arc<Context>) -> Result<()> {
    let shell_integration_result = {
        for shell in [Shell::Bash, Shell::Zsh] {
            for integration in shell.get_shell_integrations(ctx.env())? {
                integration.uninstall().await?;
            }
        }
        Ok(())
    };

    if components.contains(InstallComponents::BINARY) {
        let remove_binary = |path: PathBuf| async move {
            match tokio::fs::remove_file(&path).await {
                Ok(_) => tracing::info!("Removed binary: {path:?}"),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
                Err(err) => tracing::warn!(%err, "Failed to remove binary: {path:?}"),
            }
        };

        let folders = [directories::home_local_bin()?];

        let mut all_binary_names = vec![CLI_BINARY_NAME, CHAT_BINARY_NAME, PTY_BINARY_NAME];
        all_binary_names.extend(OLD_CLI_BINARY_NAMES);
        all_binary_names.extend(OLD_PTY_BINARY_NAMES);

        let mut pty_names = vec![PTY_BINARY_NAME];
        pty_names.extend(OLD_PTY_BINARY_NAMES);

        for folder in folders {
            for binary_name in &all_binary_names {
                let binary_path = folder.join(binary_name);
                remove_binary(binary_path).await;
            }

            for shell in Shell::all() {
                for pty_name in &pty_names {
                    let pty_path = folder.join(format!("{shell} ({pty_name})"));
                    remove_binary(pty_path).await;
                }
            }
        }
    }

    if components.contains(InstallComponents::DESKTOP_APP) {
        uninstall_desktop(&ctx).await?;
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

    shell_integration_result
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
async fn uninstall_desktop(_ctx: &Context) -> Result<()> {
    use fig_util::consts::APP_BUNDLE_ID;
    use tokio::fs;
    use tracing::{
        error,
        warn,
    };

    // Remove launch agents
    if let Ok(home) = directories::home_dir() {
        let launch_agents = home.join("Library").join("LaunchAgents");
        remove_in_dir_with_prefix_unless(&launch_agents, "com.amazon.codewhisperer.", |p| p.contains("daemon")).await;
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

    tokio::process::Command::new("defaults")
        .args(["delete", "com.amazon.codewhisperer.shared"])
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

    // Remove the previous codewhisperer data dir only if it is a symlink.
    if let Ok(old_fig_data_dir) = directories::old_fig_data_dir() {
        if old_fig_data_dir.exists() {
            if let Ok(metadata) = fs::symlink_metadata(&old_fig_data_dir).await {
                if metadata.is_symlink() {
                    fs::remove_file(&old_fig_data_dir)
                        .await
                        .map_err(|err| error!("Failed to remove the old fig data dir {old_fig_data_dir:?}: {err}"))
                        .ok();
                }
            }
        }
    }

    Ok(())
}

async fn remove_in_dir_with_prefix_unless(dir: &std::path::Path, prefix: &str, unless: impl Fn(&str) -> bool) {
    use tokio::fs;

    if let Ok(mut entries) = fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(prefix) && !unless(name) {
                    fs::remove_file(entry.path()).await.ok();
                    fs::remove_dir_all(entry.path()).await.ok();
                }
            }
        }
    }
}
