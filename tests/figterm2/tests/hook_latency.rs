//! Stage 0 latency benchmark, zsh hook path (docs/16-latency.md), next to `latency.rs`'s pty binary
//! path and measured at the same points: `key→hook` from the write to the pty master until the edit
//! buffer is decoded by the desktop's server (`fig_remote_ipc::hook`, handler entry), `key→echo`
//! until the first output. No pty binary: the terminal talks to zsh directly, and `fig.zsh` reports
//! from `zle-line-pre-redraw` over `hook.sock`.
//!
//! Ignored because it is a measurement: `just latency`, or
//! `cargo nextest run -p figterm2 --run-ignored only --no-capture -- latency_`.

use std::time::{
    Duration,
    Instant,
};

use fig_remote_ipc::figterm::FigtermCommand;
use figterm2::hook::{
    Daemon,
    HookEvent,
    HookShell,
    Rc,
};
use figterm2::latency::Summary;

/// The same keys, pace and rounds as `latency.rs`.
const TEXT: &str = "git status --short";
const ROUNDS: usize = 2;
const PACE: Duration = Duration::from_millis(50);
const TIMEOUT: Duration = Duration::from_secs(2);
const BACKSPACE: &str = "\x7f";

/// `zsh`: a path, or a bare name looked up on the test runner's `PATH` as `latency.rs` does.
async fn measure(zsh: &str) {
    let path = if zsh.contains('/') {
        zsh.into()
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join(zsh))
            .find(|path| path.is_file())
            .expect("zsh on PATH")
    };
    let label = format!("zsh-hook {}", path.display());
    let rc = "bindkey -e\nunsetopt beep\nPS1='%# '\n";
    let mut shell = HookShell::spawn_zsh(path.to_str().unwrap(), Rc::Text(rc), Daemon::Real, true)
        .await
        .unwrap();

    // As `latency.rs`: the first report of a line carries the environment; keep it out.
    shell.keystroke("x", "x", TIMEOUT).await.unwrap();
    tokio::time::sleep(PACE).await;
    shell.keystroke(BACKSPACE, "", TIMEOUT).await.unwrap();
    tokio::time::sleep(PACE).await;

    let mut inserts = Vec::new();
    let mut deletes = Vec::new();
    for _ in 0..ROUNDS {
        for (i, c) in TEXT.char_indices() {
            inserts.push(
                shell
                    .keystroke(&c.to_string(), &TEXT[..i + c.len_utf8()], TIMEOUT)
                    .await
                    .unwrap(),
            );
            tokio::time::sleep(PACE).await;
        }
        for len in (0..TEXT.len()).rev() {
            deletes.push(shell.keystroke(BACKSPACE, &TEXT[..len], TIMEOUT).await.unwrap());
            tokio::time::sleep(PACE).await;
        }
    }

    let all: Vec<_> = inserts.iter().chain(&deletes).copied().collect();
    let hooks = |t: &[(Duration, Option<Duration>)]| t.iter().map(|t| t.0).collect::<Vec<_>>();
    let echoes = |t: &[(Duration, Option<Duration>)]| t.iter().filter_map(|t| t.1).collect::<Vec<_>>();
    for (hop, keys, summary) in [
        ("key→hook", "all", Summary::of(&hooks(&all))),
        ("key→hook", "insert", Summary::of(&hooks(&inserts))),
        ("key→hook", "delete", Summary::of(&hooks(&deletes))),
        ("key→echo", "all", Summary::of(&echoes(&all))),
    ] {
        println!("latency {label} {hop:<9} {keys:<6} {}", summary.expect("no samples"));
    }
    assert!(all.len() >= 50, "only {} keystrokes measured", all.len());
}

/// The system zsh, what a new Terminal.app tab runs for a login shell of `/bin/zsh`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "benchmark: `just latency`"]
async fn latency_zsh_hook() {
    measure("/bin/zsh").await;
}

/// The zsh on `PATH`, the one `latency.rs` measures the pty binary with, so the two paths compare
/// on the same shell.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "benchmark: `just latency`"]
async fn latency_zsh_hook_path_zsh() {
    measure("zsh").await;
}

/// How long after Tab an insertion reaches zsh when the app answers at once: Tab → the intercepted
/// key at the desktop's handler, and Tab → the edit buffer with the insertion applied (the report
/// zsh sends right after `INSERT`). The app's own time — webview, suggestion, `InsertText` request
/// — is not in it: the handler answers the moment the key arrives. This is the floor under the key
/// widget's 80 ms wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "benchmark: `just latency`"]
async fn latency_zsh_hook_insert() {
    let rc = "bindkey -e\nunsetopt beep\nPS1='%# '\n";
    let mut shell = HookShell::spawn(Rc::Text(rc), Daemon::Real).await.unwrap();
    let mut to_key = Vec::new();
    let mut to_insert = Vec::new();
    for _ in 0..30 {
        shell.type_text("git ch").await.unwrap();
        shell
            .wait_for_event(
                Instant::now() + TIMEOUT,
                |e| matches!(e, HookEvent::EditBuffer { text, .. } if text == "git ch"),
            )
            .await
            .unwrap();
        for command in [
            FigtermCommand::InterceptFigJs {
                intercept_keystrokes: true,
                intercept_global_keystrokes: false,
                actions: vec![],
                override_actions: false,
            },
            FigtermCommand::InterceptFigJSVisible { visible: true },
        ] {
            shell.command(command).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        shell.events();

        let start = Instant::now();
        shell.press("\t").unwrap();
        let key = shell
            .arrival(
                TIMEOUT,
                |e| matches!(e, HookEvent::InterceptedKey { key, .. } if key == "tab"),
            )
            .await
            .unwrap();
        shell
            .command(FigtermCommand::InsertText {
                insertion: Some("\x08\x08checkout ".into()),
                deletion: None,
                offset: None,
                immediate: None,
                insertion_buffer: Some("git ch".into()),
                insert_during_command: None,
            })
            .unwrap();
        let applied = shell
            .arrival(
                TIMEOUT,
                |e| matches!(e, HookEvent::EditBuffer { text, .. } if text == "git checkout "),
            )
            .await
            .unwrap();
        to_key.push(key.saturating_duration_since(start));
        to_insert.push(applied.saturating_duration_since(start));

        shell
            .command(FigtermCommand::InterceptFigJSVisible { visible: false })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        shell.press("\x15").unwrap();
        tokio::time::sleep(PACE).await;
    }
    println!("latency zsh-hook tab→key    {}", Summary::of(&to_key).unwrap());
    println!("latency zsh-hook tab→insert {}", Summary::of(&to_insert).unwrap());
}
