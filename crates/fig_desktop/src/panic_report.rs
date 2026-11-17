//! Emergency diagnostics independent of tracing locks and its background writer.
use std::io::Write;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::atomic::{
    AtomicBool,
    Ordering,
};

const ROTATE_BYTES: u64 = 1024 * 1024;
static WRITING: AtomicBool = AtomicBool::new(false);

struct WriterGuard;
impl Drop for WriterGuard {
    fn drop(&mut self) {
        WRITING.store(false, Ordering::Release);
    }
}
fn open_report(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() >= ROTATE_BYTES) {
        std::fs::rename(path, path.with_extension("log.1"))?;
    }
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

pub fn install(path: Option<PathBuf>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Never wait on a lock from a panic hook. Concurrent/reentrant failures still reach
        // the previous stderr hook; at most one writer rotates/appends the emergency file.
        if let Some(path) = &path
            && WRITING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            let _guard = WriterGuard;
            if let Ok(mut file) = open_report(path) {
                let _ = writeln!(
                    file,
                    "pid={} thread={:?} {info}\n{}",
                    std::process::id(),
                    std::thread::current().name(),
                    std::backtrace::Backtrace::force_capture()
                );
                let _ = file.flush();
            }
        }
        // Preserve Rust's normal stderr diagnostics and panic policy. This does not recover
        // an abort, an Objective-C exception or a process killed outside the Rust runtime.
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotates_one_backup_and_creates_a_private_report() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("panic.log");
        open_report(&path).unwrap().set_len(ROTATE_BYTES).unwrap();
        open_report(&path).unwrap().write_all(b"new failure").unwrap();
        assert_eq!(
            std::fs::metadata(path.with_extension("log.1")).unwrap().len(),
            ROTATE_BYTES
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new failure");
        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
