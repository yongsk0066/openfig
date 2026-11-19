use fig_integrations::Integration;
use fig_util::directories::fig_data_dir;
use semver::Version;
use tracing::error;

const PREVIOUS_VERSION_KEY: &str = "desktop.versionAtPreviousLaunch";

/// Run items at launch
pub async fn run_install() {
    if let Err(err) = refresh_shell_scripts().await {
        error!(%err, "Failed to refresh bundled shell scripts");
    }

    if let Err(err) = set_previous_version(current_version()) {
        error!(%err, "Failed to set previous version");
    }
}

async fn refresh_shell_scripts() -> anyhow::Result<()> {
    use fig_integrations::shell::ShellExt;
    tokio::fs::create_dir_all(fig_data_dir()?.join("shell")).await?;
    for shell in fig_util::Shell::all() {
        for script in shell.get_script_integrations()? {
            script.install().await?;
        }
    }
    Ok(())
}

/// The current version of the desktop app
fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap()
}

fn set_previous_version(version: Version) -> anyhow::Result<()> {
    fig_settings::state::set_value(PREVIOUS_VERSION_KEY, version.to_string())?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_current_version() {
        current_version();
    }
}
