//! Ownership of the CLI entry in the user's bin directory.
use std::io::{
    ErrorKind,
    Result,
};
use std::path::{
    Path,
    PathBuf,
};

use fig_util::consts::system_paths::APPLICATIONS_DIR;
use fig_util::consts::{
    APP_BUNDLE_NAME,
    CLI_BINARY_NAME,
};
use tokio::fs;

pub fn installed_bundle() -> PathBuf {
    Path::new(APPLICATIONS_DIR).join(APP_BUNDLE_NAME)
}

fn cli_target() -> PathBuf {
    installed_bundle().join("Contents/MacOS").join(CLI_BINARY_NAME)
}

pub async fn is_installed() -> crate::Result<bool> {
    let link = fig_util::directories::home_local_bin()?.join(CLI_BINARY_NAME);
    let target = cli_target();
    Ok(check_link(&target, &link, false).await? && fs::try_exists(target).await?)
}

pub async fn install() -> crate::Result<()> {
    let target = cli_target();
    if !fs::try_exists(&target).await? {
        return Err(crate::Error::Custom(
            format!(
                "Move the app to {} and open it there before setting up zsh.",
                installed_bundle().display()
            )
            .into(),
        ));
    }
    let dir = fig_util::directories::home_local_bin()?;
    fs::create_dir_all(&dir).await?;
    check_link(&target, &dir.join(CLI_BINARY_NAME), true).await?;
    Ok(())
}

async fn check_link(target: &Path, link: &Path, create: bool) -> Result<bool> {
    if create {
        match fs::symlink(target, link).await {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {},
            Err(e) => return Err(e),
        }
    }
    match fs::read_link(link).await {
        Ok(existing) if existing == target => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound && !create => Ok(false),
        Err(e) if e.kind() != ErrorKind::InvalidInput => Err(e),
        _ => Err(std::io::Error::new(
            ErrorKind::AlreadyExists,
            format!(
                "{} belongs to another installation. Move it aside if you want to use this CLI.",
                link.display()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn repeated_and_concurrent_installs_preserve_foreign_entries() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("q");
        let target = Path::new("/Applications/Fig.app/Contents/MacOS/q");
        assert!(!check_link(target, &link, false).await.unwrap());
        let (a, b) = tokio::join!(check_link(target, &link, true), check_link(target, &link, true));
        assert!(a.unwrap() && b.unwrap());
        fs::remove_file(&link).await.unwrap();
        fs::symlink("another-cli", &link).await.unwrap();
        assert!(check_link(target, &link, true).await.is_err());
        assert_eq!(fs::read_link(&link).await.unwrap(), Path::new("another-cli"));
        fs::remove_file(&link).await.unwrap();
        fs::write(&link, b"original").await.unwrap();
        assert!(check_link(target, &link, true).await.is_err());
        assert_eq!(fs::read(&link).await.unwrap(), b"original");
    }
}
