//! Stage 0 latency benchmark, figterm path (docs/15-architecture-study.md §4, docs/16-latency.md).
//!
//! `key→hook`: keystroke written to the pty master → edit buffer hook decoded by the stub desktop
//! on `remote.sock`, i.e. everything upstream of the desktop's `edit_buffer` handler; what the
//! desktop and the webview do next is `scripts/latency-trace.sh`'s half. `key→echo`: the same
//! write → first output back through the pty binary. `bare echo`: that write → echo with no pty
//! binary in between, the floor the other two sit on.
//!
//! Ignored because they are measurements, not checks: they only fail when a keystroke produces no
//! hook or no echo at all. Run with `just latency` (release build), or
//! `cargo nextest run -p figterm2 --run-ignored all --no-capture -- latency_`.

use std::io::{
    Read as _,
    Write as _,
};
use std::time::{
    Duration,
    Instant,
};

use figterm2::latency::Summary;
use figterm2::{
    Desktop,
    Echo,
    KeystrokeTiming,
    Shell,
};
use portable_pty::{
    CommandBuilder,
    PtySize,
    native_pty_system,
};

/// Typed and then erased key by key, so both inserts and deletes are sampled and the line never
/// wraps.
const TEXT: &str = "git status --short";
const ROUNDS: usize = 2;
/// Gap between the hook for one key and the next write: ~120 wpm, a fast typist.
const PACE: Duration = Duration::from_millis(50);
const TIMEOUT: Duration = Duration::from_secs(2);
const BACKSPACE: &str = "\x7f";

async fn measure(shell_name: &str) {
    let mut shell = Shell::spawn(shell_name, Echo::Discard).await.unwrap();

    // The first hook after a prompt carries the whole environment. That is a real per-prompt
    // cost, but it is one key in a line, so it is kept out of the per-keystroke numbers.
    shell.keystroke("x", "x", TIMEOUT).await.unwrap();
    tokio::time::sleep(PACE).await;
    shell.keystroke(BACKSPACE, "", TIMEOUT).await.unwrap();
    tokio::time::sleep(PACE).await;

    let mut inserts: Vec<KeystrokeTiming> = Vec::new();
    let mut deletes: Vec<KeystrokeTiming> = Vec::new();
    for _ in 0..ROUNDS {
        for (i, c) in TEXT.char_indices() {
            let expected = &TEXT[..i + c.len_utf8()];
            inserts.push(shell.keystroke(&c.to_string(), expected, TIMEOUT).await.unwrap());
            tokio::time::sleep(PACE).await;
        }
        for len in (0..TEXT.len()).rev() {
            deletes.push(shell.keystroke(BACKSPACE, &TEXT[..len], TIMEOUT).await.unwrap());
            tokio::time::sleep(PACE).await;
        }
    }

    let all: Vec<KeystrokeTiming> = inserts.iter().chain(&deletes).copied().collect();
    let hooks = |t: &[KeystrokeTiming]| t.iter().map(|t| t.hook).collect::<Vec<_>>();
    let echoes = |t: &[KeystrokeTiming]| t.iter().filter_map(|t| t.echo).collect::<Vec<_>>();

    let report = [
        ("key→hook", "all", Summary::of(&hooks(&all))),
        ("key→hook", "insert", Summary::of(&hooks(&inserts))),
        ("key→hook", "delete", Summary::of(&hooks(&deletes))),
        ("key→echo", "all", Summary::of(&echoes(&all))),
    ];
    for (hop, keys, summary) in report {
        println!(
            "latency {shell_name:<4} {hop:<9} {keys:<6} {}",
            summary.expect("no samples")
        );
    }
    assert!(all.len() >= 50, "{shell_name}: only {} keystrokes measured", all.len());
}

/// The same keys into the same shell with no pty binary in between (`bash --norc`, `zsh -f`):
/// what the terminal echo costs on its own, so `key→echo` above reads as "the pty binary adds X".
async fn measure_bare(shell_name: &str) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(shell_name);
    match shell_name {
        "zsh" => cmd.args(["-f", "-i"]),
        _ => cmd.args(["--norc", "--noprofile", "-i"]),
    }
    cmd.env("HOME", tempdir.path());
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut writer = pair.master.take_writer().unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, mut output_at) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut buf = [0; 1024];
        while let Ok(n) = reader.read(&mut buf) {
            let at = Instant::now();
            if n == 0 || tx.send(at).is_err() {
                break;
            }
        }
    });

    // Wait out the first prompt: output has been quiet for 300 ms.
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(300), output_at.recv()).await {}

    let mut press = async |key: &str| -> Duration {
        while output_at.try_recv().is_ok() {}
        let start = Instant::now();
        writer.write_all(key.as_bytes()).unwrap();
        writer.flush().unwrap();
        let at = tokio::time::timeout(TIMEOUT, output_at.recv())
            .await
            .unwrap_or_else(|_| panic!("{shell_name}: no echo for {key:?}"))
            .expect("pty closed");
        tokio::time::sleep(PACE).await;
        at.saturating_duration_since(start)
    };

    let mut echoes = Vec::new();
    for _ in 0..ROUNDS {
        for c in TEXT.chars() {
            echoes.push(press(&c.to_string()).await);
        }
        for _ in 0..TEXT.len() {
            echoes.push(press(BACKSPACE).await);
        }
    }
    println!(
        "latency {shell_name:<4} {:<9} {:<6} {}",
        "bare echo",
        "all",
        Summary::of(&echoes).expect("no samples")
    );

    child.kill().ok();
    child.wait().ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "benchmark: `just latency`"]
async fn latency_bash() {
    measure("bash").await;
    measure_bare("bash").await;
}

/// zsh reports through the hook (`hook_latency.rs`); only the bare floor is measured here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "benchmark: `just latency`"]
async fn latency_zsh_bare() {
    measure_bare("zsh").await;
}

/// Drives keystrokes into an isolated desktop for `scripts/latency-trace.sh` to read back from its
/// log; measures nothing itself. Does nothing unless `FIGTERM2_DESKTOP_TMPDIR` names the `TMPDIR`
/// of a desktop started as in docs/16-latency.md:
///
/// ```sh
/// export FIGTERM2_DESKTOP_TMPDIR="$(mktemp -d)"
/// HOME="$(mktemp -d)" TMPDIR="$FIGTERM2_DESKTOP_TMPDIR" FIG_LOG_LEVEL=trace \
///     target/release/fig_desktop --allow-multiple --no-dashboard &
/// cargo nextest run --cargo-profile release -p figterm2 --run-ignored only --no-capture -- latency_desktop_drive
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "drives an isolated desktop; see docs/16-latency.md"]
async fn latency_desktop_drive() {
    let Some(tmpdir) = std::env::var_os("FIGTERM2_DESKTOP_TMPDIR") else {
        println!("latency_desktop_drive: FIGTERM2_DESKTOP_TMPDIR is not set, nothing to drive");
        return;
    };
    let shell_name = std::env::var("FIGTERM2_DESKTOP_SHELL").unwrap_or_else(|_| "bash".into());
    let mut shell = Shell::spawn_with(&shell_name, Echo::Discard, Desktop::External(tmpdir.into()))
        .await
        .unwrap();

    // Slower than `measure`: the webview has to render and ask for a position between keys, and
    // the log is joined by order, so one keystroke's hops must not overlap the next one's.
    let pace = Duration::from_millis(250);
    let mut keys = 0;
    for _ in 0..ROUNDS {
        for c in TEXT.chars() {
            shell.press(&c.to_string()).unwrap();
            keys += 1;
            tokio::time::sleep(pace).await;
        }
        for _ in 0..TEXT.len() {
            shell.press(BACKSPACE).unwrap();
            keys += 1;
            tokio::time::sleep(pace).await;
        }
    }
    println!("latency_desktop_drive: {keys} keys into {shell_name}");
}

/// Keys for [`segments`]: `TEXT` typed and erased this many times, 216 keys.
const SEGMENT_ROUNDS: usize = 6;

/// Where the pass-through time goes (docs/10-execution-plan.md open item 22): the same keystrokes
/// as [`measure`], lined up against the pty binary's own `figterm::latency` marks by wall clock.
/// Needs `FIG_LOG_LEVEL=figterm::latency=trace` in the environment, which the shell and the pty
/// binary inherit; `just latency-segments` sets it. Every other target stays at the default level,
/// so the marks are the only logging on the path.
async fn segments(shell_name: &str) {
    let level = std::env::var("FIG_LOG_LEVEL").unwrap_or_default();
    if !level.contains("figterm::latency=trace") {
        println!("segments: FIG_LOG_LEVEL=figterm::latency=trace is not set, nothing measured");
        return;
    }

    let mut shell = Shell::spawn(shell_name, Echo::Discard).await.unwrap();
    shell.keystroke("x", "x", TIMEOUT).await.unwrap();
    tokio::time::sleep(PACE).await;
    shell.keystroke(BACKSPACE, "", TIMEOUT).await.unwrap();
    tokio::time::sleep(PACE).await;

    let mut keys: Vec<KeystrokeTiming> = Vec::new();
    for _ in 0..SEGMENT_ROUNDS {
        for (i, c) in TEXT.char_indices() {
            let expected = &TEXT[..i + c.len_utf8()];
            keys.push(shell.keystroke(&c.to_string(), expected, TIMEOUT).await.unwrap());
            tokio::time::sleep(PACE).await;
        }
        for len in (0..TEXT.len()).rev() {
            keys.push(shell.keystroke(BACKSPACE, &TEXT[..len], TIMEOUT).await.unwrap());
            tokio::time::sleep(PACE).await;
        }
    }
    // The pty binary's log writer is a background thread.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut log = String::new();
    let mut logs = Vec::new();
    let mut dirs = vec![shell.tempdir.path().to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "log") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                logs.push((path, text.len()));
                log.push_str(&text);
            }
        }
    }
    let marks = figterm2::latency::parse_marks(&log);
    assert!(
        !marks.is_empty(),
        "{shell_name}: no figterm::latency marks in the pty binary's logs {logs:?}; first lines:\n{}",
        log.lines().take(5).collect::<Vec<_>>().join("\n")
    );

    let wall_us = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as u64;
    let names: Vec<String> = std::iter::once("key")
        .chain(figterm2::latency::POINTS.iter().copied())
        .collect::<Vec<_>>()
        .windows(2)
        .map(|pair| format!("{}→{}", pair[0], pair[1]))
        .chain([
            "hook_queued→stub".to_owned(),
            "key→echo".to_owned(),
            "key→hook".to_owned(),
        ])
        .collect();
    let mut samples: Vec<Vec<Duration>> = vec![Vec::new(); names.len()];
    let mut incomplete = 0;
    for (i, key) in keys.iter().enumerate() {
        let written = wall_us(key.written_at);
        let until = keys.get(i + 1).map_or(u64::MAX, |next| wall_us(next.written_at));
        let Some(points) = figterm2::latency::chain(&marks, written, until) else {
            incomplete += 1;
            continue;
        };
        let hook = written + key.hook.as_micros() as u64;
        let times: Vec<u64> = std::iter::once(written)
            .chain(points.iter().copied())
            .chain([hook])
            .collect();
        for (j, pair) in times.windows(2).enumerate() {
            samples[j].push(Duration::from_micros(pair[1].saturating_sub(pair[0])));
        }
        samples[names.len() - 2].extend(key.echo);
        samples[names.len() - 1].push(key.hook);
    }

    println!(
        "segments {shell_name}: {} keys, {incomplete} without a complete chain",
        keys.len()
    );
    println!(
        "segments {shell_name}: {:<24} {:>4} {:>9} {:>9} {:>9}",
        "segment (µs)", "n", "median", "p90", "max"
    );
    for (name, samples) in names.iter().zip(&samples) {
        let summary = Summary::of(samples).expect("no samples");
        println!(
            "segments {shell_name}: {name:<24} {:>4} {:>9} {:>9} {:>9}",
            summary.n,
            summary.median.as_micros(),
            summary.p90.as_micros(),
            summary.max.as_micros()
        );
    }
    assert!(
        keys.len() - incomplete >= 200,
        "{shell_name}: only {} complete keys",
        keys.len() - incomplete
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "benchmark: `just latency-segments`"]
async fn segments_bash() {
    segments("bash").await;
}
