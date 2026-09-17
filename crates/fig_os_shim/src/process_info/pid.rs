use std::fmt;
use std::path::PathBuf;
use std::sync::Weak;

use super::macos::{
    cmdline,
    current,
    exe,
    parent,
};
use crate::Context;

/// Represents a currently running process. [Pid] should be used as the interface to accessing
/// process-related information, e.g. the current process executable or parent processes.
#[derive(Debug, Clone)]
pub struct Pid(pub(crate) Weak<Context>, pub(crate) RawPid);

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.1)
    }
}

/// Wrapper around the platform's actual process id type.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RawPid(pub(crate) nix::libc::pid_t);

impl fmt::Display for RawPid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<nix::unistd::Pid> for RawPid {
    fn from(pid: nix::unistd::Pid) -> Self {
        RawPid(pid.as_raw())
    }
}

impl Pid {
    pub(crate) fn current(ctx: Weak<Context>) -> Self {
        current(ctx.clone())
    }

    /// Returns the parent process of this process, if it exists.
    pub fn parent(&self) -> Option<Box<Pid>> {
        parent(self.0.clone(), &self.1)
    }

    /// Returns the path to the executable file that launched this process if it exists.
    pub fn exe(&self) -> Option<PathBuf> {
        exe(self.0.clone(), &self.1)
    }

    /// Returns the command that originally started this process. Optional since not all platforms
    /// (e.g. Mac) support this.
    pub fn cmdline(&self) -> Option<String> {
        cmdline(self.0.clone(), &self.1)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn parent_name() {
        let ctx = Context::new();
        let process_pid = Pid::current(Arc::downgrade(&ctx));
        let parent_pid = process_pid.parent().unwrap();
        let parent_exe = parent_pid.exe().unwrap();
        let parent_name = parent_exe.file_name().unwrap().to_str().unwrap().to_lowercase();

        assert!(
            parent_name.contains("cargo"),
            "Unexpected parent process name: {}",
            parent_name
        );
    }
}
