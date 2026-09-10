use std::path::{
    Path,
    PathBuf,
};

use fig_util::directories;

use crate::Result;

/// Copies `path` into `backup_dir` (or today's timestamped backup directory) before it is
/// modified. Missing files are not an error.
///
/// The directory used to be resolved with `utc_backup_dir().unwrap()`. Every caller passed
/// `utc_backup_dir().ok()`, so the one case that could produce `None` — `utc_backup_dir` failing —
/// was precisely the case that then panicked, in the middle of rewriting the user's rc files.
pub(crate) fn backup_file(path: impl AsRef<Path>, backup_dir: Option<impl Into<PathBuf>>) -> Result<()> {
    let path = path.as_ref();
    let Some(name) = path.file_name() else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }

    let dir = match backup_dir {
        Some(dir) => dir.into(),
        None => directories::utc_backup_dir()?,
    };
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(path, dir.join(name))?;

    Ok(())
}
