//! The figterm nesting guard, end to end (docs/15-architecture-study.md §2.6 #10, `bcd88849`).
//!
//! `q _ should-figterm-launch` must refuse to wrap a shell that already runs inside the pty
//! binary; otherwise every new shell `exec`s another pty binary around itself. Its unit tests fake
//! the process table (`fig_os_shim::process_info::FakePid`); these build the real one — a process
//! whose executable is named `zsh (figterm)` or `figterm`, a real shell under it, `q` under that —
//! inside a real pty, and read the exit status (0 launch, 1 don't, 2 "decide by `FIG_TERM`").
//!
//! The shape that matters is the grandparent one. In real use the pty binary spawns the shell and
//! the shell runs `q`, so `q`'s *parent* is always the shell; the guard at
//! `should_figterm_launch.rs:81` only looks at the parent and never sees the pty binary. The
//! grandparent is then "not a terminal" and the answer is 2, which `pre.sh` turns into a launch
//! whenever `FIG_TERM` is missing — exactly the stale-copy case `bcd88849` was about.

use std::io::Read as _;
use std::path::{
    Path,
    PathBuf,
};

use fig_util::consts::PTY_BINARY_NAME;
use portable_pty::{
    CommandBuilder,
    PtySize,
    native_pty_system,
};
use tempfile::TempDir;

const LAUNCH: u32 = 0;
const DONT_LAUNCH: u32 = 1;
const DECIDE_BY_FIG_TERM: u32 = 2;

/// What sits above `q _ should-figterm-launch`.
#[derive(Debug, Clone, Copy)]
enum Ancestry<'a> {
    /// `<name>` → `q`
    Parent(&'a str),
    /// `<name>` → `/bin/zsh -c` → `q`: how the pty binary, the shell and the integration nest.
    Grandparent(&'a str),
    /// test runner → `/bin/zsh -c` → `q`
    Shell,
}

/// Runs `q _ should-figterm-launch` under `ancestry` in a fresh pty and returns its exit status,
/// with the command's output for assertion messages.
fn should_figterm_launch(ancestry: Ancestry<'_>) -> (u32, String) {
    let tempdir = TempDir::new().unwrap();
    let home = tempdir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let q = figterm2::cli_binary().unwrap();

    let named = |name: &str| -> PathBuf {
        let bin = tempdir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        std::fs::copy(env!("CARGO_BIN_EXE_figterm2-parent"), &path).unwrap();
        path
    };
    // `"$0"` rather than splicing the path into the script. `exit $?` keeps zsh from `exec`ing `q`
    // as its last command, which would make `q` take the shell's place in the process tree.
    let zsh_running_q = |q: &Path| -> Vec<String> {
        vec![
            "/bin/zsh".into(),
            "-c".into(),
            "\"$0\" _ should-figterm-launch; exit $?".into(),
            q.display().to_string(),
        ]
    };
    let argv: Vec<String> = match ancestry {
        Ancestry::Parent(name) => vec![
            named(name).display().to_string(),
            q.display().to_string(),
            "_".into(),
            "should-figterm-launch".into(),
        ],
        Ancestry::Grandparent(name) => std::iter::once(named(name).display().to_string())
            .chain(zsh_running_q(&q))
            .collect(),
        Ancestry::Shell => zsh_running_q(&q),
    };

    let mut cmd = CommandBuilder::from_argv(argv.iter().map(Into::into).collect());
    // Nothing from the runner's environment: `CI`, `SSH_*`, `FIG_TERM`, `TERM_PROGRAM` all change
    // the answer before the ancestry is looked at.
    cmd.env_clear();
    cmd.env("HOME", &home);
    cmd.env("PATH", "/usr/bin:/bin");
    cmd.env("TMPDIR", tempdir.path());
    cmd.cwd(&home);

    let pair = native_pty_system()
        .openpty(PtySize {
            cols: 120,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let output = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buf = [0; 1024];
        // EIO once the last process holding the slave side exits.
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            output.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&output).into_owned()
    });

    let status = child.wait().unwrap();
    drop(pair.master);
    (status.exit_code(), output.join().unwrap())
}

fn assert_status(ancestry: Ancestry<'_>, expected: impl Fn(u32) -> bool, expectation: &str) {
    let (status, output) = should_figterm_launch(ancestry);
    assert!(
        expected(status),
        "{ancestry:?}: expected {expectation}, got exit {status}\n--- output ---\n{output}"
    );
}

/// The two controls prove the fake ancestry reaches the guard at all: an unrecognised grandparent
/// gives 2, a grandparent named like a special terminal gives 0. Without them every "non-zero"
/// below could pass because the helper never ran.
#[test]
fn nesting_guard_controls() {
    assert_status(Ancestry::Shell, |s| s == DECIDE_BY_FIG_TERM, "2 (unknown grandparent)");
    assert_status(
        Ancestry::Grandparent("tmux"),
        |s| s == LAUNCH,
        "0 (tmux is a special terminal)",
    );
}

/// The parent check that exists today (`should_figterm_launch.rs:81`).
#[test]
fn nesting_guard_parent_is_pty_binary() {
    let copy = format!("zsh ({PTY_BINARY_NAME})");
    assert_status(Ancestry::Parent(&copy), |s| s == DONT_LAUNCH, "1");
    assert_status(Ancestry::Parent(PTY_BINARY_NAME), |s| s == DONT_LAUNCH, "1");
}

/// The real nesting shape must at least not say "launch".
#[test]
fn nesting_guard_grandparent_is_pty_binary_not_launch() {
    let copy = format!("zsh ({PTY_BINARY_NAME})");
    assert_status(Ancestry::Grandparent(&copy), |s| s != LAUNCH, "non-zero");
    assert_status(Ancestry::Grandparent(PTY_BINARY_NAME), |s| s != LAUNCH, "non-zero");
}

/// The real nesting shape must be refused outright, not left to `FIG_TERM`.
#[test]
fn nesting_guard_grandparent_is_pty_binary_refused() {
    let copy = format!("zsh ({PTY_BINARY_NAME})");
    assert_status(Ancestry::Grandparent(&copy), |s| s == DONT_LAUNCH, "1");
    assert_status(Ancestry::Grandparent(PTY_BINARY_NAME), |s| s == DONT_LAUNCH, "1");
}
