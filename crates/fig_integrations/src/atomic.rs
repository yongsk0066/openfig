//! Crash-safe file replacement.

use std::io;
use std::path::Path;

/// Replaces `path` with `contents` atomically.
///
/// Writes to a sibling `<name>.q.tmp`, `fsync`s it, and `rename`s it over `path`. The rename is
/// atomic within a filesystem, so a reader either sees the whole old file or the whole new one —
/// never a truncated one — and a crash or a full disk leaves the original untouched.
///
/// The obvious alternative, `File::create` + `write_all`, truncates first: an interrupted write
/// leaves an empty or half-written file. For a shell rc file that means a shell that no longer
/// starts, and there is nothing left to recover from.
///
/// `mode` is applied to the temporary file before the rename, so the replacement is never visible
/// with the wrong permissions. When it is `None` the mode of an existing `path` is preserved
/// (a fresh file gets the process umask).
pub(crate) fn write_atomic(path: impl AsRef<Path>, contents: &[u8], mode: Option<u32>) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{
        OpenOptionsExt as _,
        PermissionsExt as _,
    };

    let path = path.as_ref();
    let Some(file_name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        ));
    };

    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".q.tmp");
    let tmp_path = path.with_file_name(tmp_name);

    let mode = match mode {
        Some(mode) => Some(mode),
        None => std::fs::metadata(path).ok().map(|meta| meta.permissions().mode()),
    };

    let write = |tmp_path: &Path| -> io::Result<()> {
        let mut options = std::fs::File::options();
        options.write(true).create(true).truncate(true);
        if let Some(mode) = mode {
            options.mode(mode);
        }
        let mut file = options.open(tmp_path)?;
        file.write_all(contents)?;
        // `rename` orders the *directory* entry, not the file's data: without this a crash right
        // after the rename can leave the new name pointing at zero bytes.
        file.sync_all()?;
        drop(file);

        std::fs::rename(tmp_path, path)
    };

    write(&tmp_path).inspect_err(|_| {
        // Do not leave the half-written temp file behind for the next run to trip over.
        let _ = std::fs::remove_file(&tmp_path);
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn replaces_the_file_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".zshrc");
        std::fs::write(&path, "old\n").unwrap();

        write_atomic(&path, b"new contents\n", None).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new contents\n");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "temp file left behind"
        );
    }

    #[test]
    fn preserves_the_existing_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("integration");
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&path, b"new\n", None).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode was not preserved across the replacement");

        write_atomic(&path, b"newer\n", Some(0o755)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "explicit mode was not applied");
    }

    /// The whole point: a failed write must not damage what is already there. `File::create`
    /// truncates first, so the same failure used to leave an empty rc file behind.
    #[test]
    fn a_failed_write_leaves_the_original_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".zshrc");
        std::fs::write(&path, "important user config\n").unwrap();

        // A read-only directory makes creating the temp file fail.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let err = write_atomic(&path, b"replacement\n", None).unwrap_err();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "important user config\n");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "temp file left behind"
        );
    }
}
