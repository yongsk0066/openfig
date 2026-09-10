use std::io::ErrorKind;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs;
use tracing::debug;

use crate::Integration;
use crate::atomic::write_atomic;
use crate::error::{
    Error,
    ErrorExt,
    Result,
};

#[derive(Debug, Clone)]
pub struct FileIntegration {
    pub(crate) path: PathBuf,
    pub(crate) contents: String,
    pub(crate) mode: Option<u32>,
}

#[async_trait]
impl Integration for FileIntegration {
    fn describe(&self) -> String {
        format!("File Integration @ {}", self.path.to_string_lossy())
    }

    async fn is_installed(&self) -> Result<()> {
        // Check for parent folder permissions issues
        {
            use nix::unistd::{
                AccessFlags,
                access,
            };

            // Walk up until an ancestor is accessible, reporting the highest one that was not.
            // Same behaviour as before — including that reaching an accessible ancestor does not
            // clear an error found below it — but `ancestors()` terminates. The old `loop` only
            // ever broke from inside its `if let Some(parent)` arm, so a path whose `parent()` is
            // `None` (`/`, or a relative path with a single component) span forever at 100% CPU.
            let mut res = Ok(());
            for parent in self.path.ancestors().skip(1) {
                match access(parent, AccessFlags::R_OK | AccessFlags::W_OK | AccessFlags::X_OK) {
                    Ok(()) => break,
                    Err(err) => {
                        res = Err(Error::PermissionDenied {
                            path: parent.into(),
                            inner: err.into(),
                        });
                    },
                }
            }
            res?;
        }

        let current_contents = match fs::read_to_string(&self.path).await.with_path(&self.path) {
            Ok(contents) => contents,
            Err(Error::Io(err)) if err.kind() == ErrorKind::NotFound => {
                return Err(Error::FileDoesNotExist(self.path.clone().into()));
            },
            Err(err) => return Err(err),
        };
        if current_contents.ne(&self.contents) {
            let message = format!("{} should contain:\n{}", self.path.display(), self.contents);
            return Err(Error::ImproperInstallation(message.into()));
        }
        Ok(())
    }

    async fn install(&self) -> Result<()> {
        if self.is_installed().await.is_ok() {
            return Ok(());
        }

        let parent_dir = self
            .path
            .parent()
            .ok_or_else(|| Error::Custom("Could not get integration file directory".into()))?;

        if !parent_dir.is_dir() {
            fs::create_dir_all(&parent_dir).await.with_path(parent_dir)?;
        }

        debug!(path =? self.path, "Writing file integrations");
        // Atomic: these are sourced by the user's shell at startup, so a half-written file is a
        // shell that fails to start. Small enough (<1 KB) that the blocking write does not matter.
        write_atomic(&self.path, self.contents.as_bytes(), self.mode).with_path(&self.path)?;

        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        match fs::remove_file(&self.path).await.with_path(&self.path) {
            Ok(_) => Ok(()),
            Err(Error::Io(err)) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_integration() {
        let tempdir = tempfile::tempdir().unwrap();
        let integration = FileIntegration {
            path: tempdir.path().join("integration.txt"),
            contents: "test".into(),
            // weird mode for testing
            mode: None,
        };

        assert_eq!(
            format!("File Integration @ {}/integration.txt", tempdir.path().display()),
            integration.describe()
        );

        // ensure no integration is marked as not installed
        assert!(matches!(
            integration.is_installed().await,
            Err(Error::FileDoesNotExist(_))
        ));

        // ensure the integration can be installed
        integration.install().await.unwrap();
        assert!(integration.is_installed().await.is_ok());

        // ensure the integration can be installed while already installed
        integration.install().await.unwrap();
        assert!(integration.is_installed().await.is_ok());

        // ensure the integration can be uninstalled
        integration.uninstall().await.unwrap();
        assert!(matches!(
            integration.is_installed().await,
            Err(Error::FileDoesNotExist(_))
        ));

        // write bad data to integration file
        fs::write(&integration.path, "bad data").await.unwrap();
        assert!(matches!(
            integration.is_installed().await,
            Err(Error::ImproperInstallation(_))
        ));

        // fix integration file
        integration.install().await.unwrap();
        assert!(integration.is_installed().await.is_ok());
    }

    /// The parent-permission walk used to be a `loop` that only broke from inside its
    /// `if let Some(parent)` arm, so any path whose ancestors run out — a relative path with one
    /// component, or `/` itself — span forever. Every case here must return.
    #[tokio::test]
    async fn is_installed_terminates_for_paths_without_a_parent() {
        for path in ["integration.txt", "/", "."] {
            let integration = FileIntegration {
                path: PathBuf::from(path),
                contents: "test".into(),
                mode: None,
            };

            let result = tokio::time::timeout(std::time::Duration::from_secs(5), integration.is_installed())
                .await
                .unwrap_or_else(|_| panic!("is_installed({path:?}) did not terminate"));

            // The point is that it returns at all; none of these is a valid installation.
            assert!(result.is_err(), "is_installed({path:?}) unexpectedly succeeded");
        }
    }
}
