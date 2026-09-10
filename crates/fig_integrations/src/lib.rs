pub(crate) mod backup;
pub mod error;
pub mod file;
pub mod install;
pub mod shell;

use async_trait::async_trait;
pub(crate) use backup::backup_file;
pub use error::{
    Error,
    Result,
};
pub use file::FileIntegration;

#[async_trait]
pub trait Integration {
    fn describe(&self) -> String;
    async fn install(&self) -> Result<()>;
    async fn uninstall(&self) -> Result<()>;
    async fn is_installed(&self) -> Result<()>;
}
