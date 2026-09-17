//! Edit-buffer goldens (`figterm2::golden`, docs/15-architecture-study.md §4 stage 2).
//!
//! `golden_record` regenerates `crates/figterm/tests/ref/fig_*` and is what `just golden-record`
//! runs. The offline replay of those recordings through the pty binary's own code is
//! `crates/figterm/src/golden.rs`; the `golden_fig_*` tests here run four of the cases through the
//! real pty binary instead.

use std::time::Duration;

use figterm2::{
    Echo,
    Shell,
    golden,
};

/// Records every case, or the comma-separated `FIG_GOLDEN_CASES`. Does nothing unless
/// `FIG_GOLDEN_RECORD=1`, so `--run-ignored all` cannot rewrite committed recordings by accident.
#[test]
#[ignore = "rewrites crates/figterm/tests/ref: `just golden-record`"]
fn golden_record() {
    if std::env::var("FIG_GOLDEN_RECORD").as_deref() != Ok("1") {
        println!("golden_record: FIG_GOLDEN_RECORD=1 is not set, nothing recorded");
        return;
    }
    let only = std::env::var("FIG_GOLDEN_CASES").unwrap_or_default();
    let only: Vec<&str> = only.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    for name in &only {
        assert!(golden::case(name).is_some(), "no golden case named {name:?}");
    }

    let cli = figterm2::cli_binary().unwrap();
    for case in golden::CASES {
        if !only.is_empty() && !only.contains(&case.name) {
            continue;
        }
        let out = golden::ref_dir().join(case.name);
        golden::record(case, &cli, &out).unwrap_or_else(|err| panic!("{}: {err:?}", case.name));
        println!("golden_record: {} -> {}", case.name, out.display());
    }
}

/// No output from the pty binary for this long means the shell is done with an input.
const QUIET: Duration = Duration::from_millis(150);
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// The case's inputs typed into a real pty binary around a real zsh, set up exactly like the
/// recording: the last edit buffer hook the stub desktop receives must be the recording's final
/// expected edit buffer. This is what keeps the offline replay honest — the recorded bytes, the
/// read boundaries and `Term` without a pty must add up to what the real binary reports.
async fn golden_variant(name: &str) {
    let case = golden::case(name).expect("no such golden case");
    let expected = golden::final_edit_buffer(case)
        .unwrap()
        .expect("the case ends with an edit buffer");

    let mut shell = Shell::spawn_golden(case, Echo::Stdout).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();

    let mut last = None;
    for input in case.inputs() {
        match &input {
            golden::Input::Bytes(bytes) => shell.press(bytes).unwrap(),
            golden::Input::Resize { columns, screen_lines } => shell.resize(*columns, *screen_lines).unwrap(),
        }
        shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
        last = shell.edit_buffers().pop().or(last);
    }

    let last = last.map(|event| golden::EditBuffer {
        text: event.text,
        cursor: event.cursor,
    });
    assert_eq!(last, Some(expected), "{name}: final edit buffer at the stub desktop");
}

#[tokio::test]
async fn golden_fig_plain() {
    golden_variant("fig_plain").await;
}

#[tokio::test]
async fn golden_fig_multiline_prompt() {
    golden_variant("fig_multiline_prompt").await;
}

#[tokio::test]
async fn golden_fig_resize_reflow() {
    golden_variant("fig_resize_reflow").await;
}

#[tokio::test]
#[ignore = "figterm defect, same as figterm golden::fig_paste: the newline becomes 70 padding spaces"]
async fn golden_fig_paste() {
    golden_variant("fig_paste").await;
}
