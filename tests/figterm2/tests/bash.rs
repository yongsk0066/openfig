use figterm2::Shell;

async fn shell(shell: &str) {
    let mut shell = Shell::init(shell).await.unwrap();

    shell.typed("echo hello world").await.unwrap();
    assert_eq!(Some("echo hello world".into()), shell.buffer().await);
    shell.reset().await.unwrap();

    shell.typed(" \x08").await.unwrap();
    assert_eq!(Some("".into()), shell.buffer().await);
    shell.reset().await.unwrap();

    shell.typed("echo hello world").await.unwrap();
    shell.resize(40, 30).unwrap();
    shell.typed("111").await.unwrap();
    assert_eq!(Some("echo hello world111".into()), shell.buffer().await);
    shell.reset().await.unwrap();
}

#[tokio::test]
async fn bash() {
    shell("bash").await;
}

#[tokio::test]
async fn zsh() {
    shell("zsh").await;
}

/// A pasted multi-line script must not stop figterm from reporting the edit buffer.
///
/// This is the end-to-end shape of the F-05 self-deadlock: `EventHandler::send_event` runs inside
/// the `select!` loop that drains the main-loop channel, so a burst of prompt/preexec events could
/// park the loop on its own channel. The deterministic guard is
/// `figterm::tests::main_loop_channel_is_unbounded`; this one covers the surrounding paste path,
/// which had no coverage at all.
async fn paste_keeps_edit_buffer_alive(shell_name: &str) {
    let mut shell = Shell::init(shell_name).await.unwrap();

    shell.typed("echo before").await.unwrap();
    assert_eq!(Some("echo before".into()), shell.buffer().await);
    shell.reset().await.unwrap();

    // One write, 60 command cycles: 120+ main-loop events with no keystroke pacing between them.
    shell.pasted(&"true\n".repeat(60)).await.unwrap();

    shell.typed("echo after").await.unwrap();
    assert_eq!(
        Some("echo after".into()),
        shell.buffer().await,
        "{shell_name}: figterm stopped reporting the edit buffer after a pasted script"
    );
}

#[tokio::test]
async fn bash_paste() {
    paste_keeps_edit_buffer_alive("bash").await;
}

#[tokio::test]
async fn zsh_paste() {
    paste_keeps_edit_buffer_alive("zsh").await;
}
