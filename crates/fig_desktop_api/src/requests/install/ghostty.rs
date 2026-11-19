use std::path::PathBuf;
use std::time::Duration;

use fig_proto::fig::install_response::InstallationStatus;
use serde::Deserialize;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

static ENROLLMENT: Mutex<()> = Mutex::const_new(());

fn bundle() -> Result<PathBuf, String> {
    let path = fig_integrations::cli_link::installed_bundle();
    if macos_utils::bundle::get_bundle_path().as_ref() != Some(&path) {
        return Err(format!("Move the app to {} and open it there.", path.display()));
    }
    Ok(path)
}

fn failure(output: &std::process::Output) -> String {
    let message = String::from_utf8_lossy(&output.stderr);
    if message.trim().is_empty() {
        format!("Input method exited with {}", output.status)
    } else {
        message.trim().to_owned()
    }
}

#[derive(Deserialize)]
struct Source {
    enabled: bool,
    selected: bool,
}

pub(super) async fn status() -> Result<InstallationStatus, String> {
    let helper = bundle()?.join("Contents/Helpers/FigInputMethod.app");
    let link = fig_util::directories::home_dir()
        .map_err(|e| e.to_string())?
        .join("Library/Input Methods/FigInputMethod.app");
    match fs::read_link(&link).await {
        Ok(dest) if dest == helper => {},
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(InstallationStatus::NotInstalled),
        Ok(_) => {
            return Err(format!(
                "{} points to another input method. Inspect it before setup.",
                link.display()
            ));
        },
        Err(err) => return Err(format!("Could not read {}: {err}", link.display())),
    }
    let output = timeout(
        Duration::from_secs(5),
        Command::new(helper.join("Contents/MacOS/fig_input_method"))
            .arg("--status")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|err| format!("Input source check timed out: {err}"))?
    .map_err(|e| e.to_string())?;
    if output.status.code() == Some(4) {
        return Ok(InstallationStatus::NotInstalled);
    }
    if !output.status.success() {
        return Err(failure(&output));
    }
    let source: Source = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(if !source.enabled {
        InstallationStatus::RequiresApproval
    } else if !source.selected {
        InstallationStatus::RequiresSelection
    } else {
        InstallationStatus::Installed
    })
}

pub(super) async fn enroll() -> Result<(), String> {
    let _guard = ENROLLMENT
        .try_lock()
        .map_err(|err| format!("Input method setup is already running: {err}"))?;
    let contents = bundle()?.join("Contents");
    let output = timeout(
        Duration::from_secs(30),
        Command::new("/bin/bash")
            .env_clear()
            .env("HOME", fig_util::directories::home_dir().map_err(|e| e.to_string())?)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("CLI_BIN", fig_util::CLI_BINARY_NAME)
            .arg(contents.join("Resources/install-input-method.sh"))
            .arg(contents.join("Helpers/FigInputMethod.app"))
            .arg("--request")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|err| format!("Input method setup timed out. check its status and retry: {err}"))?
    .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure(&output))
    }
}
