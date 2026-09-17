//! A process that runs one command and exits with its status — nothing else.
//!
//! `tests/nesting.rs` copies this binary under the names the nesting guard looks for
//! (`zsh (figterm)`, `figterm`, `tmux`) so that it shows up with that executable name in the
//! ancestry of `q _ should-figterm-launch`. The guard reads the ancestor's *executable path*
//! (`proc_pidpath`), not `argv[0]`, so `exec -a` cannot fake it, and a copy of `/bin/zsh` is
//! SIGKILLed on launch (a platform binary outside the sealed system volume), hence this.

use std::process::{
    Command,
    ExitCode,
};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: figterm2-parent <program> [args...]");
        return ExitCode::from(64);
    };
    // `status()`, not `exec`: the point is to stay alive as the child's parent.
    match Command::new(&program).args(args).status() {
        Ok(status) => ExitCode::from(status.code().and_then(|code| u8::try_from(code).ok()).unwrap_or(128)),
        Err(err) => {
            eprintln!("figterm2-parent: {}: {err}", program.to_string_lossy());
            ExitCode::from(127)
        },
    }
}
