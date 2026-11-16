//! Real-shell coverage for `fig.zsh` and `fig_shell_ipc::hook`.
//!
//! Every test runs `/bin/zsh -i` in a real pty, through `init zsh pre`/`post` as a user's rc file
//! would, against a daemon on `hook.sock` (`zsh_hook_tests::hook`).

use std::time::Duration;

use fig_shell_ipc::session::ShellCommand;
use zsh_hook_tests::golden;
use zsh_hook_tests::hook::{
    Daemon,
    HookEvent,
    HookShell,
    Rc,
};

/// No terminal output for this long after an input means zsh is done with it.
const QUIET: Duration = Duration::from_millis(150);
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// zsh's own edit buffer after every step of a golden case must be the last one the hook reported
/// for that step — for all eleven, including the three a screen reader of the pty got wrong
/// (`fig_paste`, `fig_ps2`, `fig_line_taller_than_screen`).
/// The recording's `edit_buffer.json` is the expectation; the steps are replayed as recorded.
async fn golden_variant(name: &str) {
    let case = golden::case(name).expect("no such golden case");
    let expected = golden::edit_buffers(case).unwrap();

    let mut shell = HookShell::spawn(Rc::Golden(case), Daemon::Real).await.unwrap();

    let mut current = shell.edit_buffers().pop();
    let mut got = vec![current.clone()];
    for input in case.inputs() {
        match &input {
            golden::Input::Bytes(bytes) => shell.press(bytes).unwrap(),
            golden::Input::Resize { columns, screen_lines } => shell.resize(*columns, *screen_lines).unwrap(),
        }
        shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
        if let Some(last) = shell.edit_buffers().pop() {
            current = Some(last);
        }
        got.push(current.clone());
    }

    assert_eq!(got.len(), expected.len(), "{name}: step count");
    let inputs = case.inputs();
    for (step, (got, expected)) in got.iter().zip(&expected).enumerate() {
        let after = step.checked_sub(1).map(|i| &inputs[i]);
        assert_eq!(got, expected, "{name}: step {step}, after {after:?}");
    }
}

macro_rules! golden_cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                golden_variant(stringify!($name).trim_start_matches("hook_golden_")).await;
            }
        )*
    };
}

golden_cases!(
    hook_golden_fig_plain,
    hook_golden_fig_multiline_prompt,
    hook_golden_fig_rprompt,
    hook_golden_fig_emoji,
    hook_golden_fig_wrap,
    hook_golden_fig_autosuggestion,
    hook_golden_fig_scroll_off,
    hook_golden_fig_line_taller_than_screen,
    hook_golden_fig_resize_reflow,
    hook_golden_fig_ps2,
    hook_golden_fig_paste,
);

/// Every golden case is covered above.
#[test]
fn every_golden_case_has_a_hook_variant() {
    let source = include_str!("hook.rs");
    for case in golden::CASES {
        assert!(
            source.contains(&format!("hook_golden_{},", case.name)),
            "{} has no hook variant",
            case.name
        );
    }
}

/// emacs keys and a prompt with nothing in it, as the golden cases use.
const PLAIN: &str = "bindkey -e\nunsetopt beep\nPS1='%# '\n";

/// Waits for the edit buffer `text`, failing with everything that did arrive.
async fn expect_buffer(shell: &mut HookShell, text: &str) {
    let deadline = std::time::Instant::now() + STEP_TIMEOUT;
    let found = shell
        .wait_for_event(
            deadline,
            |event| matches!(event, HookEvent::EditBuffer { text: t, .. } if t == text),
        )
        .await;
    if let Err(err) = found {
        panic!("no edit buffer {text:?}: {err:#}\nevents: {:?}", shell.events());
    }
}

/// N9: `bindkey -v` after the hook armed. Keymap switches do not touch zle hook widgets, and the
/// keys stay bound in `viins`/`vicmd` explicitly, so reports go on — including the vi command-mode
/// cursor, which sits on the last character and is reported after it.
#[tokio::test]
async fn hook_keeps_reporting_after_bindkey_v() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Real).await.unwrap();
    shell.run("bindkey -v", "% ").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.events();

    shell.type_text("cd").await.unwrap();
    expect_buffer(&mut shell, "cd").await;
    shell.press("\x1b").unwrap();
    // KEYTIMEOUT is 0.4 s: ESC alone is only ESC once nothing follows.
    shell.settle(Duration::from_millis(600), STEP_TIMEOUT).await.unwrap();
    shell.press("i").unwrap();
    // Apart from the `x`, or zsh reads both before redrawing and reports once.
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.type_text("x").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    let cursors: Vec<(&str, i64)> = events
        .iter()
        .filter_map(|event| match event {
            HookEvent::EditBuffer { text, cursor } => Some((text.as_str(), *cursor)),
            _ => None,
        })
        .collect();
    // `cd`, ESC (vicmd, on the `d`: still 2, so not re-sent), `i` (before the `d`: 1), `x`.
    assert_eq!(cursors, vec![("c", 1), ("cd", 2), ("cd", 1), ("cxd", 2)], "{events:?}");
}

/// N9: a plugin replaces `zle-line-pre-redraw` with its own widget (`zle -N`) after the hook armed.
/// Reports stop for the rest of that line; the next prompt puts the hook back without dropping the
/// plugin's widget.
#[tokio::test]
async fn hook_rearms_after_a_plugin_takes_line_pre_redraw() {
    let rc = format!(
        "{PLAIN}plugin_redraws=0\nplugin() {{ (( plugin_redraws++ )) }}\n\
         clobber() {{ zle -N zle-line-pre-redraw plugin }}\nzle -N clobber\nbindkey '^X' clobber\n"
    );
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();

    shell.type_text("ab").await.unwrap();
    expect_buffer(&mut shell, "ab").await;
    shell.press("\x18").unwrap();
    shell.type_text("cd").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let clobbered = shell.events();
    assert!(
        !clobbered
            .iter()
            .any(|e| matches!(e, HookEvent::EditBuffer { text, .. } if text.starts_with("abc"))),
        "still reporting after the clobber: {clobbered:?}"
    );

    shell.press("\x15").unwrap();
    shell.press("\r").unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.type_text("echo redraws=$plugin_redraws").await.unwrap();
    expect_buffer(&mut shell, "echo redraws=$plugin_redraws").await;
    shell.press("\r").unwrap();
    shell.wait_for_output("\r\nredraws=", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    // The plugin ran for the keys of the clobbered line and for each of the 28 keys of this one.
    let output = shell.output();
    let redraws: usize = output
        .rsplit("\r\nredraws=")
        .next()
        .and_then(|rest| rest.split("\r\n").next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no count in the output:\n{output}"));
    assert!(redraws >= 28, "the plugin's widget stopped running: {redraws}");
}

/// A free `zle-line-pre-redraw` is taken directly (add-zle-hook-widget's dispatcher costs more per
/// key than the report). A plugin that joins later through add-zle-hook-widget, as
/// zsh-syntax-highlighting does, keeps the report running once, next to its own widget.
#[tokio::test]
async fn hook_shares_line_pre_redraw_with_a_later_plugin() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Real).await.unwrap();
    shell
        .run(
            "print -r -- ${widgets[zle-line-pre-redraw]}=widget",
            "\r\nuser:_fig_report=widget",
        )
        .await
        .unwrap();
    shell
        .run(
            "later_calls=0; later() { (( later_calls++ )) }; autoload -Uz add-zle-hook-widget; add-zle-hook-widget line-pre-redraw later; echo joined",
            "\r\njoined",
        )
        .await
        .unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.events();

    shell.type_text("echo calls=$later_calls").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let reports = shell
        .events()
        .into_iter()
        .filter(|e| matches!(e, HookEvent::EditBuffer { .. }))
        .count();
    assert_eq!(reports, "echo calls=$later_calls".len(), "one report per key");
    shell.press("\r").unwrap();
    shell.wait_for_output("\r\ncalls=", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let output = shell.output();
    let calls: usize = output
        .rsplit("\r\ncalls=")
        .next()
        .and_then(|rest| rest.split("\r\n").next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no count in the output:\n{output}"));
    assert!(calls >= 22, "the later plugin's widget did not run: {calls}");
}

/// No daemon at all: the shell is a plain shell — commands run, nothing about the hook reaches the
/// terminal — and the hook connects at a later prompt once a daemon is there.
#[tokio::test]
async fn hook_without_a_daemon_is_silent_and_connects_later() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Absent).await.unwrap();
    shell.run("echo fig$((6*7))ok", "fig42ok").await.unwrap();
    shell.run(DISARMED, "\r\ndisarmed").await.unwrap();
    assert_no_noise(&shell.output());
    assert!(shell.events().is_empty());

    shell.start_raw_daemon().unwrap();
    shell.run("echo next", "\r\nnext").await.unwrap();
    let deadline = std::time::Instant::now() + STEP_TIMEOUT;
    shell
        .wait_for_event(
            deadline,
            |event| matches!(event, HookEvent::Line(line) if line.starts_with("E ")),
        )
        .await
        .unwrap();
    let lines = shell.events();
    assert!(
        matches!(lines.first(), Some(HookEvent::Line(line)) if line.starts_with("HELLO 1 ")),
        "{lines:?}"
    );
}

/// A daemon speaking another protocol version: one `HELLO`, `WELCOME 0`, and the shell never asks
/// again or reports anything.
#[tokio::test]
async fn hook_refused_by_the_daemon_disarms_for_good() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Refusing).await.unwrap();
    shell.run("echo fig$((6*7))ok", "fig42ok").await.unwrap();
    shell.run("echo second", "\r\nsecond").await.unwrap();
    shell.type_text("typed").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    assert_no_noise(&shell.output());
    let events = shell.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        matches!(&events[0], HookEvent::Line(line) if line.starts_with("HELLO 1 ")),
        "{events:?}"
    );
}

/// The daemon goes away mid-session: the next write fails, the hook disarms, the shell carries on.
#[tokio::test]
async fn hook_survives_the_daemon_dying() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Raw).await.unwrap();
    shell.type_text("ab").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    assert!(shell.events().iter().any(|e| e == &HookEvent::Line("E 2 2 ab".into())));

    shell.kill_daemon().await;
    shell.press("\x15").unwrap();
    shell.run("echo fig$((6*7))ok", "fig42ok").await.unwrap();
    shell.run(DISARMED, "\r\ndisarmed").await.unwrap();
    shell.run("echo still$((1+1))", "still2").await.unwrap();
    assert_no_noise(&shell.output());
}

/// Prints `disarmed` when the hook holds no socket, without naming the variable where
/// [`assert_no_noise`] would find it in the echoed command line.
const DISARMED: &str = "v=_f; v+=ig_fd; echo ${(P)v:-disarmed}";

/// Nothing the hook could print: zsh's own errors name the builtin or say "not found".
fn assert_no_noise(output: &str) {
    for noise in [
        "zsocket",
        "_fig",
        "not found",
        "no such",
        "error",
        "Broken pipe",
        "bad fd",
    ] {
        assert!(
            !output.to_lowercase().contains(&noise.to_lowercase()),
            "{noise:?} in the terminal:\n{output}"
        );
    }
}

/// A directory with one file to complete, and a variable only this shell exports.
const WORKDIR: &str = "cd $ZDOTDIR && : > uniquefile1\nexport HOOK_TEST_VAR=from-the-shell\n";

/// What the desktop sends a session when the dropdown shows (`request/properties.rs`,
/// `webview/window.rs`).
fn show_dropdown(shell: &HookShell) {
    shell
        .command(ShellCommand::InterceptFigJs {
            intercept_keystrokes: true,
            intercept_global_keystrokes: false,
            actions: vec![],
            override_actions: false,
        })
        .unwrap();
    shell
        .command(ShellCommand::InterceptFigJSVisible { visible: true })
        .unwrap();
}

fn intercepted(events: &[HookEvent]) -> Vec<(&str, &str)> {
    events
        .iter()
        .filter_map(|event| match event {
            HookEvent::InterceptedKey { key, action } => Some((key.as_str(), action.as_str())),
            _ => None,
        })
        .collect()
}

/// `INTERCEPT 1`: Tab and ↓ reach the desktop as their actions and not zsh. `INTERCEPT 0`: Tab
/// completes again.
#[tokio::test]
async fn hook_intercepts_keys_only_while_asked() {
    let rc = format!("{PLAIN}{WORKDIR}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();
    shell.type_text("cat uniq").await.unwrap();
    expect_buffer(&mut shell, "cat uniq").await;
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.events();

    show_dropdown(&shell);
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.press("\t").unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.press("\x1b[B").unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert_eq!(
        intercepted(&events),
        vec![("tab", "insertCommonPrefix"), ("down", "navigateDown")],
        "{events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, HookEvent::EditBuffer { .. })),
        "zsh acted on an intercepted key: {events:?}"
    );

    shell
        .command(ShellCommand::InterceptFigJSVisible { visible: false })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.press("\t").unwrap();
    expect_buffer(&mut shell, "cat uniquefile1 ").await;
    assert!(intercepted(&shell.events()).is_empty());
}

/// A forwarded key waits for the desktop's verdict. `KEY 1`: taken, zsh does nothing. `KEY 0`
/// (interception ended after the shell last heard): zsh acts on the key after all. Lines that
/// cross the verdict are applied on the way.
#[tokio::test]
async fn hook_waits_for_the_desktops_verdict_on_a_key() {
    let rc = format!("{PLAIN}{WORKDIR}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Raw).await.unwrap();
    shell.type_text("cat uniq").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.send_line("INTERCEPT 1 tab enter").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.events();
    let asked = |line: &str| {
        let line = line.to_owned();
        move |event: &HookEvent| matches!(event, HookEvent::Line(l) if *l == line)
    };

    shell.press("\t").unwrap();
    shell
        .wait_for_event(std::time::Instant::now() + STEP_TIMEOUT, asked("K tab"))
        .await
        .unwrap();
    shell.send_line("KEY 1 tab").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    assert_eq!(shell.events(), vec![HookEvent::Line("K tab".into())]);

    shell.press("\t").unwrap();
    shell
        .wait_for_event(std::time::Instant::now() + STEP_TIMEOUT, asked("K tab"))
        .await
        .unwrap();
    shell.send_line("INTERCEPT 0").await.unwrap();
    shell.send_line("KEY 0 tab").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    assert_eq!(shell.events(), vec![
        HookEvent::Line("K tab".into()),
        HookEvent::Line("E 16 16 cat\\suniquefile1\\s".into())
    ]);

    // Interception is off now: Enter runs the line without asking.
    shell.press("\r").unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert!(
        events.contains(&HookEvent::Line("X cat\\suniquefile1".into())),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, HookEvent::Line(l) if l.starts_with("K "))),
        "{events:?}"
    );
}

/// A daemon that never answers a key costs at most a second, then zsh acts on it.
#[tokio::test]
async fn hook_gives_up_on_a_silent_daemon() {
    let rc = format!("{PLAIN}{WORKDIR}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Raw).await.unwrap();
    shell.type_text("cat uniq").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.send_line("INTERCEPT 1 tab").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.events();
    let pressed = std::time::Instant::now();
    shell.press("\t").unwrap();
    shell
        .wait_for_event(
            pressed + STEP_TIMEOUT,
            |e| matches!(e, HookEvent::Line(l) if l.starts_with("E ")),
        )
        .await
        .unwrap();
    let waited = pressed.elapsed();
    assert!(
        waited >= Duration::from_millis(900) && waited < Duration::from_secs(3),
        "{waited:?}"
    );
    assert_eq!(shell.events(), vec![
        HookEvent::Line("K tab".into()),
        HookEvent::Line("E 16 16 cat\\suniquefile1\\s".into())
    ]);
}

/// `InsertText` as the autocomplete app sends it: backspaces over the typed prefix, the suggestion
/// (an emoji in it), arrows back to `{cursor}`, and a newline to run it.
#[tokio::test]
async fn hook_applies_insertions() {
    let rc = format!("{PLAIN}{WORKDIR}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();
    let insert = |insertion: &str, buffer: &str| ShellCommand::InsertText {
        insertion: Some(insertion.into()),
        deletion: None,
        offset: None,
        immediate: None,
        insertion_buffer: Some(buffer.into()),
        insert_during_command: None,
    };

    shell.type_text("git ch").await.unwrap();
    expect_buffer(&mut shell, "git ch").await;
    shell.command(insert("\x08\x08checkout ", "git ch")).unwrap();
    expect_buffer(&mut shell, "git checkout ").await;

    shell
        .command(insert("-m=\"\u{1F600}\"\x1b[D", "git checkout "))
        .unwrap();
    expect_buffer(&mut shell, "git checkout -m=\"\u{1F600}\"").await;
    let cursor = shell.events().into_iter().rev().find_map(|event| match event {
        HookEvent::EditBuffer { cursor, .. } => Some(cursor),
        _ => None,
    });
    assert_eq!(
        cursor,
        Some("git checkout -m=\"\u{1F600}".len() as i64),
        "cursor before the closing quote"
    );

    // Asked to run the line, but not in answer to a key: typed, left for the user to run (a `zle
    // -F` handler cannot run it, see `fig_shell_ipc::hook::wire`).
    shell.press("\x15").unwrap();
    shell.type_text("echo in").await.unwrap();
    expect_buffer(&mut shell, "echo in").await;
    shell.command(insert("serted$((1+1))\n", "echo in")).unwrap();
    expect_buffer(&mut shell, "echo inserted$((1+1))").await;
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    assert!(!shell.events().contains(&HookEvent::PreExec));
    shell.press("\r").unwrap();
    shell.wait_for_output("\r\ninserted2", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert!(events.contains(&HookEvent::PreExec), "{events:?}");
    assert!(
        events.contains(&HookEvent::PostExec {
            command: Some("echo inserted$((1+1))".into()),
            exit_code: Some(0)
        }),
        "{events:?}"
    );
}

/// Sets up a raw daemon that intercepts Tab, presses Tab, and returns once the shell asked.
async fn tab_asked(daemon_rc: &str) -> HookShell {
    let rc = format!("{PLAIN}{daemon_rc}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Raw).await.unwrap();
    shell.send_line("INTERCEPT 1 tab").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.events();
    shell.press("\t").unwrap();
    shell
        .wait_for_event(std::time::Instant::now() + STEP_TIMEOUT, |e| {
            e == &HookEvent::Line("K tab".into())
        })
        .await
        .unwrap();
    shell.events();
    shell
}

fn ran(events: &[HookEvent]) -> bool {
    events.contains(&HookEvent::Line("X echo\\sran".into()))
}

/// `KEY 1` then an `INSERT` with accept: the key widget is still waiting, so the line runs and the
/// prompt comes back. Without accept the text is typed and not run.
#[tokio::test]
async fn hook_runs_an_insertion_that_answers_a_key() {
    let mut shell = tab_asked("").await;
    shell.send_line("KEY 1 tab").await.unwrap();
    shell.send_line("INSERT 0 echo\\sran 0 1").await.unwrap();
    shell.wait_for_output("\r\nran\r\n", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert!(
        events.contains(&HookEvent::Line("E 8 8 echo\\sran".into())),
        "{events:?}"
    );
    assert!(ran(&events), "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HookEvent::Line(l) if l.starts_with("P 0 "))),
        "{events:?}"
    );

    let mut shell = tab_asked("").await;
    shell.send_line("KEY 1 tab").await.unwrap();
    shell.send_line("INSERT 0 echo\\sran 0 0").await.unwrap();
    shell.settle(Duration::from_millis(400), STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert_eq!(events, vec![HookEvent::Line("E 8 8 echo\\sran".into())]);
    assert!(!shell.output().contains("\r\nran"));
}

/// The wait is bounded: an insertion after the window, or after `KEY 2` (a key that inserts
/// nothing), is typed by the `zle -F` handler and not run.
#[tokio::test]
async fn hook_does_not_run_an_insertion_outside_the_window() {
    let mut shell = tab_asked("").await;
    shell.send_line("KEY 1 tab").await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    shell.send_line("INSERT 0 echo\\sran 0 1").await.unwrap();
    shell.settle(Duration::from_millis(400), STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert_eq!(events, vec![HookEvent::Line("E 8 8 echo\\sran".into())]);

    let mut shell = tab_asked("").await;
    shell.send_line("KEY 2 tab").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    shell.send_line("INSERT 0 echo\\sran 0 1").await.unwrap();
    shell.settle(Duration::from_millis(400), STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert_eq!(events, vec![HookEvent::Line("E 8 8 echo\\sran".into())]);
    assert!(!shell.output().contains("\r\nran"));
}

/// End to end through the desktop's server: Tab is `insertCommonPrefix` (so `KEY 1`), and an app
/// that answers with an insertion ending in a newline gets the line run.
#[tokio::test]
async fn hook_runs_an_app_insertion_that_ends_in_enter() {
    let rc = format!("{PLAIN}{WORKDIR}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();
    shell.type_text("echo r").await.unwrap();
    expect_buffer(&mut shell, "echo r").await;
    show_dropdown(&shell);
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.events();
    shell.press("\t").unwrap();
    shell
        .arrival(
            STEP_TIMEOUT,
            |e| matches!(e, HookEvent::InterceptedKey { key, .. } if key == "tab"),
        )
        .await
        .unwrap();
    shell
        .command(ShellCommand::InsertText {
            insertion: Some("\x08ran$((40+2))\n".into()),
            deletion: None,
            offset: None,
            immediate: None,
            insertion_buffer: Some("echo r".into()),
            insert_during_command: None,
        })
        .unwrap();
    shell.wait_for_output("\r\nran42", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let events = shell.events();
    assert!(
        events.contains(&HookEvent::PostExec {
            command: Some("echo ran$((40+2))".into()),
            exit_code: Some(0)
        }),
        "{events:?}"
    );
}

/// `INSERT` on the wire: delete before, a multi-line text with an emoji, cursor moved back.
#[tokio::test]
async fn hook_inserts_multi_line_text_verbatim() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Raw).await.unwrap();
    shell.type_text("abc").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.events();
    shell.send_line("INSERT 1 one\\ntwo\\s\u{1F600} 1").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    // "ab" + "one\ntwo 😀", the cursor before the emoji: 10 characters, 10 bytes.
    assert_eq!(shell.events(), vec![HookEvent::Line(
        "E 10 10 abone\\ntwo\\s\u{1F600}".into()
    )]);
}

/// An insertion that arrives while a command runs is not typed into the next prompt: the desktop
/// drops it, and so does the shell if one got through.
#[tokio::test]
async fn hook_drops_insertions_while_a_command_runs() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Raw).await.unwrap();
    shell.type_text("sleep 1; echo slept").await.unwrap();
    shell.press("\r").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.send_line("INSERT 0 stale 0").await.unwrap();
    shell.wait_for_output("\r\nslept", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.events();
    shell.type_text("x").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    assert_eq!(shell.events(), vec![HookEvent::Line("E 1 1 x".into())]);

    let rc = format!("{PLAIN}{WORKDIR}");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();
    shell.type_text("sleep 1; echo slept").await.unwrap();
    shell.press("\r").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell
        .command(ShellCommand::InsertText {
            insertion: Some("stale".into()),
            deletion: None,
            offset: None,
            immediate: None,
            insertion_buffer: None,
            insert_during_command: None,
        })
        .unwrap();
    shell.wait_for_output("\r\nslept", STEP_TIMEOUT).await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.type_text("x").await.unwrap();
    expect_buffer(&mut shell, "x").await;
    assert!(
        !shell
            .events()
            .iter()
            .any(|e| matches!(e, HookEvent::EditBuffer { text, .. } if text.contains("stale")))
    );
}

/// `RunProcess` for a hook session runs in the desktop with the directory and environment the shell
/// reported.
#[tokio::test]
async fn hook_runs_processes_where_the_shell_is() {
    let rc = format!("{PLAIN}{WORKDIR}");
    let shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();
    let (command, response) = ShellCommand::run_process(
        "sh".into(),
        vec!["-c".into(), "pwd -P; echo $HOOK_TEST_VAR".into()],
        None,
        vec![],
        None,
    );
    shell.command(command).unwrap();
    let response = tokio::time::timeout(STEP_TIMEOUT, response).await.unwrap().unwrap();
    let zdotdir = shell.zdotdir.canonicalize().unwrap();
    assert_eq!(
        response.unwrap().stdout,
        format!("{}\nfrom-the-shell\n", zdotdir.display())
    );
    assert_eq!(shell.sessions(), 1);
}

/// `name` on the test runner's `PATH`.
fn on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// The owner's plugins, read-only: oh-my-zsh from `$HOME/.oh-my-zsh` (menu-select completion,
/// `up-line-or-beginning-search` on ↑, its own `zle-line-init`), then starship's prompt and fzf's
/// widgets (^R, ^T, `**` Tab) if they are installed, in the order `~/.zshrc` loads them. The same
/// keys go to a shell with the hook armed and to one where it never connects; zsh's own buffer
/// after every redraw must be the same in both. Skips when oh-my-zsh is not there.
#[tokio::test]
async fn hook_changes_nothing_with_oh_my_zsh_starship_and_fzf() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let omz = home.join(".oh-my-zsh");
    if !omz.join("oh-my-zsh.sh").is_file() {
        println!("skipping: no oh-my-zsh at {}", omz.display());
        return;
    }
    let starship = on_path("starship");
    let fzf = on_path("fzf");
    println!("loading oh-my-zsh, starship: {starship:?}, fzf: {fzf:?}");

    let mut rc = format!(
        "export ZSH='{omz}'\nZSH_THEME=\nZSH_CACHE_DIR=$ZDOTDIR/omz-cache\nDISABLE_AUTO_UPDATE=true\n\
         zstyle ':omz:update' mode disabled\nsource $ZSH/oh-my-zsh.sh\n\
         HISTFILE=$ZDOTDIR/.zsh_history\nprint -s 'echo first'; print -s 'echo second'\n",
        omz = omz.display()
    );
    for tool in [&starship, &fzf].into_iter().flatten() {
        rc.push_str(&format!("export PATH=\"{}:$PATH\"\n", tool.parent().unwrap().display()));
    }
    if starship.is_some() {
        rc.push_str("export STARSHIP_CONFIG=$ZDOTDIR/starship.toml\neval \"$(starship init zsh)\"\n");
    }
    if fzf.is_some() {
        rc.push_str("eval \"$(fzf --zsh)\"\n");
    }
    rc.push_str(
        "cd $ZDOTDIR && : > aa1 && : > aa2 && : > aa3\n\
         autoload -Uz add-zle-hook-widget\n\
         _zle_log() { print -r -- \"$BUFFER|$CURSOR\" >> $ZDOTDIR/zle.log; return 0 }\n\
         add-zle-hook-widget line-pre-redraw _zle_log\n",
    );

    let fzf_wait = Duration::from_millis(if fzf.is_some() { 700 } else { 0 });
    let mut steps: Vec<(&str, Duration)> = vec![
        // Menu selection: list, select, move twice, accept the selection, run.
        ("ls aa", Duration::ZERO),
        ("\t", Duration::ZERO),
        ("\t", Duration::ZERO),
        ("\x1b[B", Duration::ZERO),
        ("\x1b[B", Duration::ZERO),
        ("\r", Duration::ZERO),
        ("\r", Duration::ZERO),
        // History prefix search on ↑.
        ("echo", Duration::ZERO),
        ("\x1b[A", Duration::ZERO),
        ("\x1b[A", Duration::ZERO),
        ("\x15", Duration::ZERO),
        ("(exit 3)", Duration::ZERO),
        ("\r", Duration::ZERO),
    ];
    if fzf.is_some() {
        steps.extend([
            ("\x12", fzf_wait),
            ("second", fzf_wait),
            ("\r", fzf_wait),
            ("\x15", Duration::ZERO),
            ("\x14", fzf_wait),
            ("\x1b", fzf_wait),
            ("\x15", Duration::ZERO),
            ("cat aa**", Duration::ZERO),
            ("\t", fzf_wait),
            ("\r", fzf_wait),
            ("\x15", Duration::ZERO),
        ]);
    }

    let mut logs = Vec::new();
    for daemon in [Daemon::Real, Daemon::Absent] {
        let mut shell = HookShell::spawn(Rc::Text(&rc), daemon).await.unwrap();
        for (keys, extra) in &steps {
            shell.type_text(keys).await.unwrap();
            shell
                .settle(Duration::from_millis(250) + *extra, Duration::from_secs(20))
                .await
                .unwrap();
        }
        let log = std::fs::read_to_string(shell.zdotdir.join("zle.log")).unwrap();
        let events = shell.events();
        if daemon == Daemon::Real {
            assert!(
                events.contains(&HookEvent::PostExec {
                    command: Some("(exit 3)".into()),
                    exit_code: Some(3)
                }),
                "{events:?}"
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, HookEvent::EditBuffer { text, .. } if text == "ls aa"))
            );
        } else {
            assert!(events.is_empty());
        }
        logs.push(log);
    }
    assert_eq!(logs[0], logs[1], "zsh behaved differently with the hook armed");
    // And the plugins did what they do: menu selection moved, ↑ kept searching the same prefix.
    let lines: Vec<&str> = logs[0].lines().collect();
    for expected in ["ls aa2|6", "ls aa3|6", "echo second|11", "echo first|10"] {
        assert!(
            lines.contains(&expected),
            "{expected:?} missing from the buffer log:\n{}",
            logs[0]
        );
    }
    if fzf.is_some() {
        assert!(
            lines.contains(&"cat aa1 |8"),
            "fzf's ** completion did not complete:\n{}",
            logs[0]
        );
    }
}

/// Options users set that change how unquoted expansions and subscripts behave: every function of
/// `fig.zsh` but the key widget runs under `emulate -L zsh`, and the key widget touches nothing
/// they change.
#[tokio::test]
async fn hook_works_under_hostile_options() {
    let rc = format!("{PLAIN}setopt ksharrays shwordsplit extendedglob nobareglobqual\n");
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Raw).await.unwrap();
    shell.type_text("echo a  b").await.unwrap();
    shell.send_line("INSERT 1 \\s\u{e9} 1").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.send_line("INTERCEPT 1 tab").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    shell.press("\t").unwrap();
    shell
        .wait_for_event(std::time::Instant::now() + STEP_TIMEOUT, |e| {
            e == &HookEvent::Line("K tab".into())
        })
        .await
        .unwrap();
    shell.send_line("KEY 1 tab").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let lines: Vec<String> = shell
        .events()
        .into_iter()
        .filter_map(|e| match e {
            HookEvent::Line(line) if line.starts_with("E ") || line.starts_with("K ") => Some(line),
            _ => None,
        })
        .collect();
    // "echo a  " + " é", the cursor moved back before the é: 9 characters, 9 bytes.
    assert_eq!(lines[lines.len() - 3..], [
        "E 9 9 echo\\sa\\s\\sb".to_owned(),
        "E 9 9 echo\\sa\\s\\s\\s\u{e9}".to_owned(),
        "K tab".to_owned()
    ]);
    assert_no_noise(&shell.output());
}

/// The shell, not just the socket queue, acknowledges a revision-checked Unicode edit.
#[tokio::test]
async fn hook_atomic_edit_acknowledges_and_rejects_stale_input() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Real).await.unwrap();
    shell.type_text("echo 한😀").await.unwrap();
    expect_buffer(&mut shell, "echo 한😀").await;
    let revision = shell
        .shell_sessions
        .as_ref()
        .unwrap()
        .most_recent()
        .unwrap()
        .context
        .as_ref()
        .unwrap()
        .input_revision
        .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    shell
        .command(ShellCommand::ApplyEdit {
            expires_at: std::time::Instant::now() + std::time::Duration::from_millis(1500),
            revision,
            cursor_utf16: "echo 한😀".encode_utf16().count() as u64,
            buffer: "echo 한😀".into(),
            edit: fig_shell_ipc::hook::wire::Insert {
                delete_before: 2,
                text: "완료".into(),
                ..Default::default()
            },
            response: tx,
        })
        .unwrap();
    expect_buffer(&mut shell, "echo 완료").await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    shell
        .command(ShellCommand::ApplyEdit {
            expires_at: std::time::Instant::now() + std::time::Duration::from_millis(1500),
            revision,
            cursor_utf16: 8,
            buffer: "echo 한😀".into(),
            edit: fig_shell_ipc::hook::wire::Insert {
                text: "WRONG".into(),
                ..Default::default()
            },
            response: tx,
        })
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    shell.type_text("!").await.unwrap();
    expect_buffer(&mut shell, "echo 완료!").await;
}

/// A raw daemon does no validation: rejection/deduplication must happen inside the real zle owner.
#[tokio::test]
async fn hook_owner_checks_cursor_deadline_and_duplicate_edits() {
    use fig_shell_ipc::hook::wire::ShellLine;
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Raw).await.unwrap();
    shell.run("_fig_v2=1", "% ").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.events();
    shell.type_text("한😀").await.unwrap();
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    let revision = shell
        .events()
        .iter()
        .rev()
        .find_map(|event| match event {
            HookEvent::Line(line) => match ShellLine::parse(line) {
                Ok(ShellLine::EditBufferV2 { revision, text, .. }) if text == "한😀" => Some(revision),
                _ => None,
            },
            _ => None,
        })
        .expect("negotiated revision report");
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 10_000;
    shell
        .send_line(&format!(
            "INSERT2 wrong-cursor {revision} 1 한😀 0 WRONG 0 0 {deadline}"
        ))
        .await
        .unwrap();
    shell
        .wait_for_event(
            std::time::Instant::now() + STEP_TIMEOUT,
            |e| matches!(e, HookEvent::Line(line) if line == "ACK wrong-cursor 1"),
        )
        .await
        .unwrap();
    shell
        .send_line(&format!("INSERT2 expired {revision} 2 한😀 0 WRONG 0 0 0"))
        .await
        .unwrap();
    shell
        .wait_for_event(
            std::time::Instant::now() + STEP_TIMEOUT,
            |e| matches!(e, HookEvent::Line(line) if line == "ACK expired 1"),
        )
        .await
        .unwrap();
    let edit = format!("INSERT2 once {revision} 2 한😀 1 완료 0 0 {deadline}");
    shell.send_line(&edit).await.unwrap();
    shell
        .wait_for_event(
            std::time::Instant::now() + STEP_TIMEOUT,
            |e| matches!(e, HookEvent::Line(line) if line == "ACK once 0"),
        )
        .await
        .unwrap();
    shell.events();
    shell.send_line(&edit).await.unwrap();
    shell
        .wait_for_event(
            std::time::Instant::now() + STEP_TIMEOUT,
            |e| matches!(e, HookEvent::Line(line) if line == "ACK once 0"),
        )
        .await
        .unwrap();
    shell.events();
    shell.type_text("!").await.unwrap();
    shell
        .wait_for_event(std::time::Instant::now() + STEP_TIMEOUT, |e| match e {
            HookEvent::Line(line) => {
                matches!(ShellLine::parse(line), Ok(ShellLine::EditBufferV2 { text, .. }) if text == "한완료!")
            },
            _ => false,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn hook_rejects_cancelled_and_expired_edits_before_dispatch() {
    let mut shell = HookShell::spawn(Rc::Text(PLAIN), Daemon::Real).await.unwrap();
    shell.type_text("echo before").await.unwrap();
    expect_buffer(&mut shell, "echo before").await;
    let revision = shell
        .shell_sessions
        .as_ref()
        .unwrap()
        .most_recent()
        .unwrap()
        .context
        .as_ref()
        .unwrap()
        .input_revision
        .unwrap();
    for expired in [false, true] {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let rx = if expired {
            Some(rx)
        } else {
            drop(rx);
            None
        };
        shell
            .command(ShellCommand::ApplyEdit {
                expires_at: if expired {
                    std::time::Instant::now() - Duration::from_secs(1)
                } else {
                    std::time::Instant::now() + Duration::from_secs(1)
                },
                revision,
                cursor_utf16: 11,
                buffer: "echo before".into(),
                edit: fig_shell_ipc::hook::wire::Insert {
                    text: "WRONG".into(),
                    ..Default::default()
                },
                response: tx,
            })
            .unwrap();
        if let Some(rx) = rx {
            assert!(rx.await.unwrap().is_err());
        }
    }
    shell.settle(QUIET, STEP_TIMEOUT).await.unwrap();
    shell.type_text("!").await.unwrap();
    expect_buffer(&mut shell, "echo before!").await;
}
