use std::path::PathBuf;
use std::time::{
    Duration,
    Instant,
};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use fig_desktop::native::{
    ICON_TIMEOUT,
    Icon,
    Process,
    Reply,
    Request,
};

#[tokio::test]
async fn real_helper_renders_png_reuses_process_and_reaps_on_shutdown() {
    let mut process = Process::new(PathBuf::from(env!("CARGO_BIN_EXE_fig_native_worker")));
    for icon in [
        Icon::Named("txt".into()),
        Icon::Path("/Applications/Ghostty.app".into()),
        Icon::Path("/missing/folder/".into()),
    ] {
        let Reply::Icon(Some(data)) = process.call(Request::Icon(icon), Duration::from_secs(5)).await.unwrap() else {
            panic!("missing icon")
        };
        let bytes = STANDARD.decode(data).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(!bytes.is_empty());
    }
    let pid = process.pid().unwrap();
    let start = Instant::now();
    process
        .call(Request::Icon(Icon::Named("txt".into())), ICON_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(process.pid(), Some(pid));
    assert!(start.elapsed() < ICON_TIMEOUT);
    process.shutdown().await;
    assert_eq!(process.pid(), None);
    // SAFETY: signal zero only checks existence, and the child was owned and reaped above.
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}

#[tokio::test]
async fn helper_exits_if_its_parent_dies_while_its_main_thread_is_waiting() {
    use tokio::io::{
        AsyncBufReadExt,
        BufReader,
    };
    let script = "import subprocess,sys,time\np=subprocess.Popen([sys.argv[1],'--stdio'],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\nprint(p.pid,flush=True)\ntime.sleep(30)";
    let mut parent = tokio::process::Command::new("/usr/bin/python3")
        .args(["-c", script, env!("CARGO_BIN_EXE_fig_native_worker")])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(parent.stdout.take().unwrap()).lines();
    let pid: i32 = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    parent.kill().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        // SAFETY: signal zero checks existence of the helper owned by this fixture.
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stopped_real_icon_helper_is_replaced_while_the_runtime_remains_responsive() {
    let mut process = Process::new(PathBuf::from(env!("CARGO_BIN_EXE_fig_native_worker")));
    let request = || Request::Icon(Icon::Named("txt".into()));
    process.call(request(), Duration::from_secs(5)).await.unwrap();
    let old_pid = process.pid().unwrap();
    // SAFETY: this test owns the child and will kill/reap it through Process, even on failure.
    assert_eq!(unsafe { libc::kill(old_pid as i32, libc::SIGSTOP) }, 0);
    {
        let stalled = process.call(request(), Duration::from_millis(150));
        tokio::pin!(stalled);
        tokio::select! {
            _ = &mut stalled => panic!("stopped process unexpectedly replied before the heartbeat"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        }
        assert!(stalled.await.is_err());
    }
    assert!(matches!(
        process.call(request(), Duration::from_secs(5)).await.unwrap(),
        Reply::Icon(Some(_))
    ));
    assert_ne!(process.pid(), Some(old_pid));
    process.shutdown().await;
}
