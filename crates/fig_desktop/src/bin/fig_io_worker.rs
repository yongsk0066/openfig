//! Disposable I/O executor. Only an inherited-pipe request from the owning process is accepted.
fn main() -> std::process::ExitCode {
    use nix::sys::signal::{
        Signal,
        killpg,
    };
    use nix::unistd::{
        getpgrp,
        getpid,
        getppid,
    };
    let parent = getppid();
    let group = getpgrp();
    if std::env::args().nth(1).as_deref() != Some("--stdio") || parent.as_raw() <= 1 || group != getpid() {
        return std::process::ExitCode::FAILURE;
    }
    if std::thread::Builder::new()
        .name("parent-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if getppid() != parent {
                    let _ = killpg(group, Signal::SIGKILL);
                }
            }
        })
        .is_err()
    {
        return std::process::ExitCode::FAILURE;
    }
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .and_then(|runtime| runtime.block_on(fig_shell_ipc::process::serve()));
    if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
