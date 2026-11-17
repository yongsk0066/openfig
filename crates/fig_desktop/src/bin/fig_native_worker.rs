//! The desktop supervises deadlines; this guard also prevents orphaning after abrupt parent exit.
fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() != Some("--stdio") {
        return std::process::ExitCode::FAILURE;
    }
    // SAFETY: getppid has no memory preconditions.
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return std::process::ExitCode::FAILURE;
    }
    if std::thread::Builder::new()
        .name("parent-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                // SAFETY: getppid has no memory preconditions; _exit deliberately skips potentially
                // stuck native destructors after the supervising desktop has gone away.
                unsafe {
                    if libc::getppid() != parent {
                        libc::_exit(0);
                    }
                }
            }
        })
        .is_err()
    {
        return std::process::ExitCode::FAILURE;
    }
    match fig_desktop::native::serve() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
