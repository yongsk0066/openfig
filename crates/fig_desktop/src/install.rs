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

/// Hidden sibling `.<name>.new` of `dst`, where a replacement is staged before it is renamed into
/// place. Same directory, so the `rename` never crosses a file system.
fn staging_path(dst: &std::path::Path) -> Result<std::path::PathBuf, std::io::Error> {
    let name = dst.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no file name", dst.display()),
        )
    })?;
    Ok(dst.with_file_name(format!(".{}.new", name.to_string_lossy())))
}

/// Move the staged `tmp` over `dst`, or clean it up if that fails.
///
/// `rename(2)` swaps the directory entry in one step: a shell that starts at that moment finds
/// either the old file or the new one, never nothing, and a process still running from the old
/// file keeps its inode.
async fn rename_into_place(tmp: &std::path::Path, dst: &std::path::Path) -> Result<(), std::io::Error> {
    let result = tokio::fs::rename(tmp, dst).await;
    if result.is_err() {
        tokio::fs::remove_file(tmp).await.ok();
    }
    result
}

/// Symlink, and overwrite if it already exists and is invalid or not a symlink
///
/// The replacement is staged next to `dst` and renamed over it instead of removing `dst` first,
/// so `~/.local/bin/<cli>` never disappears while a new shell's rc hook is looking for it.
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
        },
        Err(err) if err.kind() == ErrorKind::NotFound => {},
        Err(err) => return Err(err),
    }

    // Not there, not a symlink, or pointing elsewhere: stage the right link and swap it in.
    let tmp = staging_path(dst)?;
    tokio::fs::remove_file(&tmp).await.ok();
    tokio::fs::symlink(src, &tmp).await?;
    rename_into_place(&tmp, dst).await
}

pub async fn initialize_fig_dir(env: &fig_os_shim::Env) -> anyhow::Result<()> {
    use std::fs;

    use fig_integrations::shell::ShellExt;
    use fig_util::consts::{
        APP_BUNDLE_ID,
        APP_PROCESS_NAME,
        CLI_BINARY_NAME,
        LAUNCH_AGENT_LABEL,
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

        // A regular file in the way is replaced by the link
        let plain = tmp_dir.join("plain");
        std::fs::write(&plain, "not a link").unwrap();
        symlink(&src_file_1, &plain).await.unwrap();
        assert_eq!(plain.read_link().unwrap(), src_file_1);
        assert!(!tmp_dir.join(".plain.new").exists());
    }
}
