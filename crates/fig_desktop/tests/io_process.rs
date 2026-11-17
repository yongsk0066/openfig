use std::path::PathBuf;
use std::time::Duration;

use fig_proto::fig::{
    EnvironmentVariable,
    RunProcessResponse,
};
use fig_shell_ipc::process::{
    Request,
    Service,
    Shell,
};
async fn run(request: Request, shell: Shell) -> Result<RunProcessResponse, String> {
    Service::new(PathBuf::from(env!("CARGO_BIN_EXE_fig_io_worker")))
        .run(request, shell)
        .await
}
mod tests {
    use super::*;

    fn env(key: &str, value: &str) -> EnvironmentVariable {
        EnvironmentVariable {
            key: key.into(),
            value: Some(value.into()),
        }
    }

    fn stdout(response: Result<RunProcessResponse, String>) -> String {
        response.unwrap_or_else(|error| panic!("{error}")).stdout
    }

    #[tokio::test]
    async fn runs_in_the_shells_directory_with_the_shells_environment() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let response = run(
            Request {
                executable: "sh".into(),
                arguments: vec![
                    "-c".into(),
                    "pwd; echo \"$FROM_SHELL $FROM_REQUEST ${HOME-unset}\"".into(),
                ],
                working_directory: None,
                env: vec![env("FROM_REQUEST", "request")],
                timeout: None,
            },
            Shell {
                env: vec![env("PATH", "/usr/bin:/bin"), env("FROM_SHELL", "shell")],
                cwd: Some(cwd.to_string_lossy().into_owned()),
            },
        )
        .await;
        // Only the shell's environment: the desktop's `HOME` is not inherited.
        assert_eq!(stdout(response), format!("{}\nshell request unset\n", cwd.display()));
    }

    #[tokio::test]
    async fn the_request_directory_wins_and_a_missing_one_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let pwd = |working_directory: Option<String>, shell_cwd: Option<String>| {
            run(
                Request {
                    executable: "/bin/pwd".into(),
                    arguments: vec![],
                    working_directory,
                    env: vec![],
                    timeout: None,
                },
                Shell {
                    env: vec![],
                    cwd: shell_cwd,
                },
            )
        };
        assert_eq!(
            stdout(pwd(Some("/".into()), Some(cwd.to_string_lossy().into())).await),
            "/\n"
        );
        assert_eq!(
            stdout(pwd(Some("/does/not/exist".into()), Some(cwd.to_string_lossy().into())).await),
            format!("{}\n", cwd.display())
        );
    }

    #[tokio::test]
    async fn failures_are_errors() {
        let missing = run(
            Request {
                executable: "/does/not/exist".into(),
                arguments: vec![],
                working_directory: None,
                env: vec![],
                timeout: None,
            },
            Shell { env: vec![], cwd: None },
        )
        .await;
        assert!(missing.is_err(), "{missing:?}");

        let slow = run(
            Request {
                executable: "/bin/sleep".into(),
                arguments: vec!["5".into()],
                working_directory: None,
                env: vec![],
                timeout: Some(Duration::from_millis(50)),
            },
            Shell { env: vec![], cwd: None },
        )
        .await;
        assert!(
            matches!(&slow, Err(message) if message.contains("timed out")),
            "{slow:?}"
        );
    }
    #[tokio::test]
    async fn dropping_work_kills_shell_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("leaked");
        let ready = dir.path().join("ready");
        let work = tokio::spawn(run(
            Request {
                executable: "/bin/sh".into(),
                arguments: vec![
                    "-c".into(),
                    "(sleep 0.5; touch \"$1\") & touch \"$2\"; wait".into(),
                    "test".into(),
                    marker.to_string_lossy().into(),
                    ready.to_string_lossy().into(),
                ],
                working_directory: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
            },
            Shell { env: vec![], cwd: None },
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        work.abort();
        assert!(work.await.unwrap_err().is_cancelled());
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!marker.exists(), "a cancelled shell left its child running");
    }

    #[tokio::test]
    async fn oversized_output_is_rejected_without_waiting_for_the_deadline() {
        let result = run(
            Request {
                executable: "/usr/bin/yes".into(),
                arguments: vec![],
                working_directory: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
            },
            Shell { env: vec![], cwd: None },
        )
        .await;
        assert!(result.unwrap_err().contains("exceeds 1 MiB"));
    }
}

#[tokio::test]
async fn real_directory_worker_preserves_names_and_reports_errors() {
    let service = Service::new(env!("CARGO_BIN_EXE_fig_io_worker").into());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("line\nbreak"), "").unwrap();
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("folder"), dir.path().join("link")).unwrap();
    let mut names = service.directory(dir.path().to_owned(), true).await.unwrap();
    names.sort();
    assert_eq!(names, ["folder/", "line\nbreak", "link/"]);
    assert!(
        service
            .directory(dir.path().join("missing"), false)
            .await
            .unwrap_err()
            .contains("Failed listing directory")
    );
}

#[tokio::test]
async fn helper_refuses_to_run_inside_the_callers_process_group() {
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_fig_io_worker"))
        .arg("--stdio")
        .stdin(std::process::Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(!status.success());
}
