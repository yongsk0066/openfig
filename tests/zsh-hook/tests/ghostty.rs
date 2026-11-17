//! Opt-in test against an unmodified upstream checkout, never the user's terminal UI.
use std::time::{
    Duration,
    Instant,
};

use fig_shell_ipc::session::ShellCommand;
use zsh_hook_tests::hook::{
    Daemon,
    HookEvent,
    HookShell,
    Rc,
};
const TIMEOUT: Duration = Duration::from_secs(5);
async fn buffer(shell: &mut HookShell, expected: &str) {
    shell
        .wait_for_event(
            Instant::now() + TIMEOUT,
            |event| matches!(event, HookEvent::EditBuffer { text, .. } if text == expected),
        )
        .await
        .unwrap();
}
async fn coexist(late: bool) {
    let path = std::env::var("GHOSTTY_SHELL_INTEGRATION")
        .expect("set GHOSTTY_SHELL_INTEGRATION to the upstream zsh/ghostty-integration file");
    assert!(std::path::Path::new(&path).is_file());
    let source = format!("export TERM_PROGRAM=ghostty; source '{}'", path.replace('\'', "'\\''"));
    let plain = "bindkey -e\nPROMPT='%% '\nRPROMPT=''\n";
    let rc = if late {
        plain.to_owned()
    } else {
        format!("{plain}{source}\n")
    };
    let mut shell = HookShell::spawn(Rc::Text(&rc), Daemon::Real).await.unwrap();
    if late {
        shell.run(&source, "\x1b]133;A").await.unwrap();
        shell.settle(Duration::from_millis(150), TIMEOUT).await.unwrap();
    }
    shell.events();
    shell.type_text("git ch").await.unwrap();
    buffer(&mut shell, "git ch").await;
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
            expires_at: Instant::now() + TIMEOUT,
            revision,
            cursor_utf16: 6,
            buffer: "git ch".into(),
            edit: fig_shell_ipc::hook::wire::Insert {
                delete_before: 2,
                text: "checkout ".into(),
                ..Default::default()
            },
            response: tx,
        })
        .unwrap();
    assert!(tokio::time::timeout(TIMEOUT, rx).await.unwrap().unwrap().is_ok());
    buffer(&mut shell, "git checkout ").await;
    shell.press("\x7f\x7f").unwrap();
    buffer(&mut shell, "git checkou").await;
    shell.press("\x15").unwrap();
    shell.events();
    shell
        .run("printf '\\nGHOSTTY-HOOK-OK\\n'", "\r\nGHOSTTY-HOOK-OK\r\n")
        .await
        .unwrap();
    buffer(&mut shell, "").await;
    // OSC prompt markers prove the real Ghostty hook ran as well as OpenFig's IPC hook.
    assert!(shell.output().contains("\x1b]133;A"));
    assert_eq!(shell.sessions(), 1);
}
#[tokio::test]
#[ignore = "requires GHOSTTY_SHELL_INTEGRATION pointing at a separately cloned upstream script"]
async fn ghostty_hook_before_fig() {
    coexist(false).await;
}
#[tokio::test]
#[ignore = "requires GHOSTTY_SHELL_INTEGRATION pointing at a separately cloned upstream script"]
async fn ghostty_hook_after_fig() {
    coexist(true).await;
}
