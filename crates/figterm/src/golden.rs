//! Edit-buffer goldens, replayed without a pty (docs/15-architecture-study.md §4 stage 2).
//!
//! `tests/ref/<case>/` holds what a real zsh wrote to its pty while `figterm2::golden` typed into
//! it (`just golden-record`): the raw bytes (`alacritty.recording`), the read boundaries, inputs
//! and resizes in order (`events.json`), and zsh's own `$PREBUFFER$BUFFER`/`$CURSOR` after every
//! input (`edit_buffer.json`). The layout is upstream alacritty's `tests/ref/<case>/`, which
//! survives in `crates/alacritty_terminal/tests/ref`.
//!
//! The replay runs every recorded read through the main loop's own path — [`advance_pty_output`],
//! [`can_send_edit_buffer`], [`send_edit_buffer`] — and decodes the `EditBufferHook`s that come out
//! of the channel. The state a step is compared on is the last hook sent up to the next input:
//! what the desktop is looking at when the user presses the next key. A step that sent nothing
//! leaves the previous hook in place, which is also what the desktop sees.
//!
//! A case that fails here means the pty binary reports a different edit buffer than the shell
//! holds. Such a case keeps zsh's value in `edit_buffer.json` and is `#[ignore]`d with the reason
//! instead of being re-recorded to match.

use std::fmt::Write as _;
use std::path::{
    Path,
    PathBuf,
};

use alacritty_terminal::Term;
use alacritty_terminal::ansi::Processor;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::SizeInfo;
use fig_proto::local::EditBufferHook;
use fig_proto::remote::{
    Hostbound,
    hostbound,
};
use serde_json::{
    Value,
    json,
};

use crate::{
    EDIT_BUFFER_TEST_LOCK,
    advance_pty_output,
    can_send_edit_buffer,
    forget_last_sent_edit_buffer,
    send_edit_buffer,
};

fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ref")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn as_usize(value: &Value, what: &str) -> usize {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or_else(|| panic!("{what} is not a count: {value}"))
}

fn edit_buffer_hook(message: Hostbound) -> Option<EditBufferHook> {
    match message.packet {
        Some(hostbound::Packet::Request(hostbound::Request {
            request: Some(hostbound::request::Request::EditBuffer(hook)),
            ..
        })) => Some(hook),
        _ => None,
    }
}

/// The recorded case replayed into the shape of its `edit_buffer.json`.
async fn replay(dir: &Path) -> Vec<Value> {
    let size = read_json(&dir.join("size.json"));
    let config = read_json(&dir.join("config.json"));
    let events = read_json(&dir.join("events.json"));
    let recording = std::fs::read(dir.join("alacritty.recording")).expect("alacritty.recording");

    let size = SizeInfo::new(
        as_usize(&size["screen_lines"], "screen_lines"),
        as_usize(&size["columns"], "columns"),
    );
    let session_id = config["session_id"]
        .as_str()
        .expect("config.json: session_id")
        .to_owned();
    let mut term = Term::new(
        size,
        VoidListener,
        as_usize(&config["history_size"], "history_size"),
        session_id,
    );
    let mut processor = Processor::new();

    crate::ipc::set_desktop_connected(true);
    forget_last_sent_edit_buffer();
    let (sender, hooks) = flume::unbounded();

    let mut steps = Vec::new();
    let mut after = Value::Null;
    let mut last_hook = Value::Null;
    let mut offset = 0;
    for event in events.as_array().expect("events.json is an array") {
        if let Some(len) = event.get("output") {
            let end = offset + as_usize(len, "output");
            advance_pty_output(&mut processor, &mut term, &recording[offset..end]);
            offset = end;

            if let Some(edit_buffer) = can_send_edit_buffer(&term) {
                send_edit_buffer(term.shell_state(), edit_buffer, &sender, None)
                    .await
                    .expect("the receiver is alive");
            }
            for hook in hooks.drain().filter_map(edit_buffer_hook) {
                last_hook = json!({ "text": hook.text, "cursor": hook.cursor });
            }
            continue;
        }

        steps.push(json!({ "after": after, "edit_buffer": last_hook }));
        let mut input = event.clone();
        input.as_object_mut().expect("event is an object").remove("t_us");
        if let Some(resize) = input.get("resize") {
            // The main loop's `InputEvent::Resized` arm.
            term.resize(SizeInfo::new(
                as_usize(&resize["screen_lines"], "screen_lines"),
                as_usize(&resize["columns"], "columns"),
            ));
        } else {
            assert!(input.get("input").is_some(), "unknown event {event}");
        }
        after = input;
    }
    steps.push(json!({ "after": after, "edit_buffer": last_hook }));
    assert_eq!(
        offset,
        recording.len(),
        "events.json does not account for every recorded byte"
    );

    crate::ipc::set_desktop_connected(false);
    steps
}

async fn golden_test(name: &str) {
    let _globals = EDIT_BUFFER_TEST_LOCK.lock().await;
    let dir = ref_dir().join(name);
    let expected = read_json(&dir.join("edit_buffer.json"));
    let expected = expected.as_array().expect("edit_buffer.json is an array");
    let actual = replay(&dir).await;

    if expected != &actual {
        let mut report = format!("{name}: the edit buffer differs from zsh's\n");
        for (i, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
            let mark = if expected == actual { " " } else { "x" };
            writeln!(
                report,
                "{mark} {i:>3} after {}\n        zsh    {}\n        figterm {}",
                expected["after"], expected["edit_buffer"], actual["edit_buffer"]
            )
            .unwrap();
        }
        if expected.len() != actual.len() {
            writeln!(
                report,
                "step count: expected {}, replayed {}",
                expected.len(),
                actual.len()
            )
            .unwrap();
        }
        panic!("{report}");
    }
}

macro_rules! golden_tests {
    ($($(#[$attr:meta])* $name:ident)*) => {
        $(
            $(#[$attr])*
            #[tokio::test]
            async fn $name() {
                golden_test(stringify!($name)).await;
            }
        )*

        const CASES: &[&str] = &[$(stringify!($name)),*];
    };
}

golden_tests! {
    fig_plain
    fig_multiline_prompt
    fig_rprompt
    fig_emoji
    fig_wrap
    fig_autosuggestion
    fig_scroll_off
    fig_resize_reflow
    #[ignore = "figterm defect: once the line is taller than the screen zsh draws a window of it marked \
                `>....`/`<....`, and figterm reports that window as the buffer (step 33: \">....\" + 13 spaces \
                + the tail, cursor 98 for 118)"]
    fig_line_taller_than_screen
    #[ignore = "figterm defect: in a PS2 continuation the newline and the PS2 prompt cells become padding \
                spaces (step 11: 77 spaces between the lines, cursor 87 for 11)"]
    fig_ps2
    #[ignore = "figterm defect: a newline in a pasted buffer becomes padding to the end of the row \
                (step 1: \"echo one\" + 70 spaces + \"echo two\", cursor 86 for 17)"]
    fig_paste
}

/// Every recorded case has a test above and every test a recording.
#[test]
fn every_recording_has_a_test() {
    let mut recorded: Vec<String> = std::fs::read_dir(ref_dir())
        .expect("tests/ref")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    recorded.sort();
    let mut tests: Vec<&str> = CASES.to_vec();
    tests.sort_unstable();
    assert_eq!(recorded, tests);
}
