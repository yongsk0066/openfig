mod pid;

use std::path::PathBuf;
use std::sync::{
    Arc,
    Weak,
};

pub use pid::Pid;

use crate::Context;

mod macos;

/// Represents the interface to accessing info about the currently running process tree.
#[derive(Debug, Clone)]
pub struct ProcessInfo(Weak<Context>);

impl ProcessInfo {
    /// Creates a new [ProcessInfo]. This takes a [Weak] pointer since accessing process info on
    /// platforms like Linux requires file system access, and allows this instance to be embedded
    /// within a single [Context] instance.
    pub(crate) fn new(ctx: Weak<Context>) -> Self {
        Self(ctx)
    }

    /// Returns a [Pid] representing the currently running process.
    pub fn current_pid(&self) -> Pid {
        Pid::current(self.0.clone())
    }
}

/// Returns the [MyPid::exe] of the current process's parent, with extra logic to account for
/// `toolbox-exec`.
pub fn get_parent_process_exe(ctx: &Arc<Context>) -> Option<PathBuf> {
    let mut pid = Pid::current(Arc::downgrade(ctx));
    loop {
        pid = *pid.parent()?;
        match pid.exe() {
            // We ignore toolbox-exec since we never want to know if that is the parent process
            Some(pid) if pid.file_name().is_some_and(|s| s == "toolbox-exec") => {},
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_parent_process_exe() {
        let ctx = Context::new();
        get_parent_process_exe(&ctx);
    }
}
