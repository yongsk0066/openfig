//! A pty binary refused for its protocol version (`fig_proto::remote::PROTOCOL_VERSION`) must leave
//! the user with a working shell.
//!
//! The case is a terminal tab that was opened before a rebuild: its `<shell> (figterm)` copy is
//! from the old build and the desktop refuses it. The shell must keep echoing and running commands
//! through the pty binary with no hooks, and the pty binary must not keep reconnecting — the
//! answer cannot change while the same binary runs.

use std::time::{
    Duration,
    Instant,
};

use fig_proto::remote::PROTOCOL_VERSION;
use figterm2::{
    Desktop,
    Echo,
    Shell,
};

/// The reconnect interval in `figterm/src/ipc.rs` is 5 s; waiting past it shows whether a refusal
/// is retried.
const PAST_ONE_RECONNECT: Duration = Duration::from_millis(6500);

async fn refused_handshake_keeps_the_shell_working(shell_name: &str) {
    // Returns once the first handshake reached the stub and was refused.
    let mut shell = Shell::spawn_with(shell_name, Echo::Stdout, Desktop::Refusing)
        .await
        .unwrap();
    let refused_at = Instant::now();

    // Output that only running the command produces: the echoed command line reads `$((6*7))`.
    shell.typed("echo fig$((6*7))ok\n").await.unwrap();
    shell
        .wait_for_output("fig42ok", Duration::from_secs(10))
        .await
        .unwrap_or_else(|err| panic!("{shell_name}: the command did not run: {err:#}"));

    shell.typed("echo still$((1+1))\n").await.unwrap();
    shell
        .wait_for_output("still2", Duration::from_secs(10))
        .await
        .unwrap_or_else(|err| panic!("{shell_name}: the second command did not run: {err:#}"));

    tokio::time::sleep(PAST_ONE_RECONNECT.saturating_sub(refused_at.elapsed())).await;

    // Still the pty binary in the middle, still in raw mode: nothing crashed or fell back to exec.
    assert!(!shell.has_exited(), "{shell_name}: the pty binary exited");
    assert!(
        !shell.is_cooked(),
        "{shell_name}: the pty binary is no longer running the terminal"
    );

    shell.typed("echo after$((2+3))\n").await.unwrap();
    shell
        .wait_for_output("after5", Duration::from_secs(10))
        .await
        .unwrap_or_else(|err| panic!("{shell_name}: the shell stopped after the reconnect window: {err:#}"));

    let refusals = shell.refusals.lock().await;
    assert_eq!(
        refusals.handshakes,
        [PROTOCOL_VERSION],
        "{shell_name}: expected exactly one handshake carrying this build's version"
    );
    assert_eq!(
        refusals.other_packets, 0,
        "{shell_name}: a refused pty binary sent hooks"
    );
}

#[tokio::test]
async fn bash_refused_handshake_keeps_the_shell_working() {
    refused_handshake_keeps_the_shell_working("bash").await;
}
