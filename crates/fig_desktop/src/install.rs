use fig_integrations::Integration;
use fig_util::directories::fig_data_dir;
use macos_utils::bundle::get_bundle_path_for_executable;
use semver::Version;
use tracing::error;

const PREVIOUS_VERSION_KEY: &str = "desktop.versionAtPreviousLaunch";

/// Run items at launch
pub async fn run_install() {
    initialize_fig_dir(&fig_os_shim::Env::new()).await.ok();

    if let Err(err) = set_previous_version(current_version()) {
        error!(%err, "Failed to set previous version");
    }
}

/// Run `launchctl` without blocking the caller's thread.
///
/// `run_install` is awaited from `main` before the event loop starts, so a synchronous
/// `Command::status()` here delays window creation by however long launchctl takes.
/// Failures are expected (`load` on an already-loaded agent errors) and only logged.
async fn launchctl<'a>(args: impl IntoIterator<Item = &'a str>) {
    let mut command = tokio::process::Command::new("launchctl");
    command.args(args);
    if let Err(err) = command.status().await {
        tracing::debug!(%err, "launchctl failed");
    }
}

/// True when both paths exist, have the same length and identical bytes.
async fn same_file_contents(a: &std::path::Path, b: &std::path::Path) -> bool {
    let (Ok(ma), Ok(mb)) = (tokio::fs::metadata(a).await, tokio::fs::metadata(b).await) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    match (tokio::fs::read(a).await, tokio::fs::read(b).await) {
        (Ok(da), Ok(db)) => da == db,
        _ => false,
    }
}

/// Symlink, and overwrite if it already exists and is invalid or not a symlink
async fn symlink(src: impl AsRef<std::path::Path>, dst: impl AsRef<std::path::Path>) -> Result<(), std::io::Error> {
    use std::io::ErrorKind;

    let src = src.as_ref();
    let dst = dst.as_ref();

    // Check if the link already exists
    match tokio::fs::symlink_metadata(dst).await {
        Ok(metadata) => {
            // If it's a symlink, check if it points to the right place
            if metadata.file_type().is_symlink()
                && let Ok(read_link) = tokio::fs::read_link(dst).await
                && read_link == src
            {
                return Ok(());
            }

            // If it's not a symlink or it points to the wrong place, delete it
            tokio::fs::remove_file(dst).await?;
        },
        Err(err) if err.kind() == ErrorKind::NotFound => {},
        Err(err) => return Err(err),
    }

    // Create the symlink
    tokio::fs::symlink(src, dst).await
}

pub async fn initialize_fig_dir(env: &fig_os_shim::Env) -> anyhow::Result<()> {
    use std::fs;

    use fig_integrations::shell::ShellExt;
    use fig_util::Shell;
    use fig_util::consts::{
        APP_BUNDLE_ID,
        APP_PROCESS_NAME,
        CLI_BINARY_NAME,
        LAUNCH_AGENT_LABEL,
        PTY_BINARY_NAME,
    };
    use macos_utils::bundle::get_bundle_path;

    use crate::launchd_plist::{
        LaunchdPlist,
        create_launch_agent,
    };

    let local_bin = fig_util::directories::home_local_bin()?;
    if let Err(err) = fs::create_dir_all(&local_bin) {
        error!(%err, "Failed to create {local_bin:?}");
    }

    // Install figterm to ~/.local/bin
    match get_bundle_path_for_executable(PTY_BINARY_NAME) {
        Some(pty_path) => {
            let link = local_bin.join(PTY_BINARY_NAME);
            if let Err(err) = symlink(&pty_path, link).await {
                error!(%err, "Failed to symlink for {PTY_BINARY_NAME}: {pty_path:?}");
            }

            for shell in Shell::all() {
                let pty_shell_cpy = local_bin.join(format!("{shell} ({PTY_BINARY_NAME})"));
                let pty_path = pty_path.clone();

                tokio::spawn(async move {
                    // Refresh the copy whenever its bytes differ from the bundled binary. The
                    // upstream check compared `--version` strings, which never change between
                    // local rebuilds of the same version — so every rebuild kept spawning the
                    // copy from the first install, and once the env-var names moved that stale
                    // figterm made `should-figterm-launch` start another figterm inside it.
                    if same_file_contents(&pty_shell_cpy, &pty_path).await {
                        return;
                    }

                    // On a first install there is nothing to remove; ENOENT here is the normal
                    // path, not an error worth two ERROR lines in every fresh log.
                    if let Err(err) = tokio::fs::remove_file(&pty_shell_cpy).await
                        && err.kind() != std::io::ErrorKind::NotFound
                    {
                        error!(%err, "Failed to remove {PTY_BINARY_NAME} shell {shell:?} copy");
                    }
                    if let Err(err) = tokio::fs::copy(&pty_path, &pty_shell_cpy).await {
                        error!(%err, "Failed to copy {PTY_BINARY_NAME} to {}", pty_shell_cpy.display());
                    }
                });
            }
        },
        None => error!("Failed to find {PTY_BINARY_NAME} in bundle"),
    }

    // install the cli to ~/.local/bin
    match get_bundle_path_for_executable(CLI_BINARY_NAME) {
        Some(cli_path) => {
            let dest = local_bin.join(CLI_BINARY_NAME);
            if let Err(err) = symlink(&cli_path, dest).await {
                error!(%err, "Failed to symlink {CLI_BINARY_NAME}");
            }
        },
        None => error!("Failed to find {CLI_BINARY_NAME} in bundle"),
    }

    if let Some(bundle_path) = get_bundle_path() {
        let exe = bundle_path.join("Contents").join("MacOS").join(APP_PROCESS_NAME);
        let startup_launch_agent = LaunchdPlist::new(LAUNCH_AGENT_LABEL)
            .program_arguments([&exe.to_string_lossy(), "--is-startup", "--no-dashboard"])
            .associated_bundle_identifiers([APP_BUNDLE_ID])
            .run_at_load(true);
        let path = startup_launch_agent.get_file_path()?;

        // The plist used to be written unconditionally, so turning `app.launchOnStartup` off
        // still spawned the app at login just to have `main.rs:106` exit immediately.
        if fig_settings::settings::get_bool_or("app.launchOnStartup", true) {
            create_launch_agent(&startup_launch_agent)?;
            launchctl(["load", path.as_str()]).await;
        } else if path.exists() {
            launchctl(["unload", path.as_str()]).await;
            if let Err(err) = tokio::fs::remove_file(&path).await
                && err.kind() != std::io::ErrorKind::NotFound
            {
                error!(%err, "Failed to remove the login launch agent");
            }
        }
    }

    // Init the shell directory
    std::fs::create_dir(fig_data_dir()?.join("shell")).ok();
    for shell in fig_util::Shell::all().iter() {
        for script_integration in shell.get_script_integrations().unwrap_or_default() {
            if let Err(err) = script_integration.install().await {
                error!(%err, "Failed installing shell integration {}", script_integration.describe());
            }
        }

        for shell_integration in shell.get_shell_integrations(env).unwrap_or_default() {
            if let Err(err) = shell_integration.install().await {
                error!(%err, "Failed installing shell integration {}", shell_integration.describe());
            }
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

    #[tokio::test]
    async fn test_symlink() {
        use tempfile::tempdir;

        let tmp_dir = tempdir().unwrap();
        let tmp_dir = tmp_dir.path();

        // folders
        let src_dir_1 = tmp_dir.join("dir_1");
        let src_dir_2 = tmp_dir.join("dir_2");
        let dst_dir = tmp_dir.join("dst");

        std::fs::create_dir_all(&src_dir_1).unwrap();
        std::fs::create_dir_all(&src_dir_2).unwrap();

        // Check that a new symlink is created
        assert!(!dst_dir.exists());
        symlink(&src_dir_1, &dst_dir).await.unwrap();
        assert!(dst_dir.exists());
        assert_eq!(dst_dir.read_link().unwrap(), src_dir_1);

        // Check that the symlink is updated
        symlink(&src_dir_2, &dst_dir).await.unwrap();
        assert!(dst_dir.exists());
        assert_eq!(dst_dir.read_link().unwrap(), src_dir_2);

        // files
        let src_file_1 = src_dir_1.join("file_1");
        let src_file_2 = src_dir_2.join("file_2");
        let dst_file = dst_dir.join("file");

        std::fs::write(&src_file_1, "content 1").unwrap();
        std::fs::write(&src_file_2, "content 2").unwrap();

        // Check that a new symlink is created
        assert!(!dst_file.exists());
        symlink(&src_file_1, &dst_file).await.unwrap();
        assert!(dst_file.exists());
        assert_eq!(std::fs::read_to_string(&dst_file).unwrap(), "content 1");

        // Check that the symlink is updated
        symlink(&src_file_2, &dst_file).await.unwrap();
        assert!(dst_file.exists());
        assert_eq!(std::fs::read_to_string(&dst_file).unwrap(), "content 2");
    }
}
