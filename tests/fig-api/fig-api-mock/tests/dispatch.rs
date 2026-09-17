//! Every `ClientOriginatedMessage` request variant through `fig-api-mock`, recorded byte for byte.
//!
//! This is the safety net for reshaping the request dispatch (docs/15-architecture-study.md §2.3
//! B): the snapshots were recorded before the refactor, and the refactor is only correct if every
//! one of them still matches. Each transcript line pairs the base64 of the encoded
//! `ServerOriginatedMessage` (the bytes the webview receives) with its decoded form for reading.
//!
//! The mock binary is run as a child process rather than called in process, so that each case can
//! get its own `HOME`: the settings file, the state database and the history database all live
//! under `$HOME/Library/Application Support`, and the requests that read or write them must never
//! touch the real ones. The child's environment is cleared apart from `HOME`, so the recording does
//! not depend on the caller's (`CI=1` included).
//!
//! The seven requests only the desktop app can answer (window, figterm session, event loop) reach
//! the mock's own handler here; `crates/fig_desktop/src/protocol/api.rs` records what the desktop
//! itself answers for six of them (not `UpdateApplicationPropertiesRequest`, see there).

use std::fmt::Write as _;
use std::fs;
use std::path::{
    Path,
    PathBuf,
};
use std::process::Command;

use base64::prelude::*;
use fig_proto::fig::client_originated_message::Submessage;
use fig_proto::fig::history_query_request::Param;
use fig_proto::fig::history_query_request::param::Type as ParamType;
use fig_proto::fig::insert_text_request::Type as InsertTextType;
use fig_proto::fig::{
    AppendToFileRequest,
    ClientOriginatedMessage,
    ContentsOfDirectoryRequest,
    CreateDirectoryRequest,
    DestinationOfSymbolicLinkRequest,
    DragWindowRequest,
    FilePath,
    GetLocalStateRequest,
    GetPlatformInfoRequest,
    GetSettingsPropertyRequest,
    HistoryQueryRequest,
    InsertTextRequest,
    InstallAction,
    InstallComponent,
    InstallRequest,
    NotificationRequest,
    NotificationType,
    OnboardingAction,
    OnboardingRequest,
    OpenInExternalApplicationRequest,
    PingRequest,
    PositionWindowRequest,
    ReadFileRequest,
    RunProcessRequest,
    ServerOriginatedMessage,
    UpdateApplicationPropertiesRequest,
    UpdateLocalStateRequest,
    UpdateSettingsPropertyRequest,
    WriteFileRequest,
    append_to_file_request,
    write_file_request,
};
use fig_proto::prost::Message;

/// A scratch `HOME` and working directory for one case, and the transcript of what was sent.
struct Sandbox {
    root: tempfile::TempDir,
    transcript: String,
    next_id: i64,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("home")).unwrap();
        fs::create_dir(root.path().join("cwd")).unwrap();
        Self {
            root,
            transcript: String::new(),
            next_id: 1,
        }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn cwd(&self) -> PathBuf {
        self.root.path().join("cwd")
    }

    /// Sends `submessage` with the next message id and records the exchange.
    fn send(&mut self, submessage: Submessage) {
        let id = self.next_id;
        self.next_id += 1;
        self.send_message(ClientOriginatedMessage {
            id: Some(id),
            submessage: Some(submessage),
        });
    }

    fn send_message(&mut self, request: ClientOriginatedMessage) {
        let request_b64 = BASE64_STANDARD.encode(request.encode_to_vec());
        let output = Command::new(env!("CARGO_BIN_EXE_fig-api-mock"))
            .env_clear()
            .env("HOME", self.home())
            .arg("request")
            .arg("--cwd")
            .arg(self.cwd())
            .arg(&request_b64)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fig-api-mock failed for {request:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let response_b64 = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        let response =
            ServerOriginatedMessage::decode(BASE64_STANDARD.decode(&response_b64).unwrap().as_slice()).unwrap();

        writeln!(self.transcript, "> {request:?}").unwrap();
        writeln!(self.transcript, "< {response_b64}").unwrap();
        writeln!(self.transcript, "< {response:?}").unwrap();
    }

    /// Records a fact about the sandbox after the requests, e.g. what a write left on disk.
    fn note(&mut self, fact: impl std::fmt::Display) {
        writeln!(self.transcript, "# {fact}").unwrap();
    }

    fn note_file(&mut self, path: &Path) {
        let shown = self.relative(path);
        match fs::read(path) {
            Ok(bytes) => self.note(format!("{shown}: {:?}", String::from_utf8_lossy(&bytes))),
            Err(err) => self.note(format!("{shown}: {}", err.kind())),
        }
    }

    fn note_dir(&mut self, path: &Path) {
        let shown = self.relative(path);
        self.note(format!("{shown} is a directory: {}", path.is_dir()));
    }

    fn relative(&self, path: &Path) -> String {
        format!("{}", path.strip_prefix(self.root.path()).unwrap().display())
    }

    fn transcript(self) -> String {
        self.transcript
    }
}

fn file_path(path: &str) -> Option<FilePath> {
    Some(FilePath {
        path: path.into(),
        relative_to: None,
        expand_tilde_in_path: None,
    })
}

#[test]
fn ping() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::PingRequest(PingRequest {}));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn get_platform_info() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::GetPlatformInfoRequest(GetPlatformInfoRequest {}));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn missing_submessage() {
    let mut sandbox = Sandbox::new();
    sandbox.send_message(ClientOriginatedMessage {
        id: Some(1),
        submessage: None,
    });
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn insert_text() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::InsertTextRequest(InsertTextRequest {
        r#type: Some(InsertTextType::Text("echo".into())),
        terminal_session_id: None,
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn notification() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::NotificationRequest(NotificationRequest {
        subscribe: Some(true),
        r#type: Some(NotificationType::NotifyOnSettingsChange.into()),
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn run_process() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::RunProcessRequest(RunProcessRequest {
        executable: "true".into(),
        ..Default::default()
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn update_application_properties() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::UpdateApplicationPropertiesRequest(
        UpdateApplicationPropertiesRequest {
            intercept_bound_keystrokes: Some(true),
            ..Default::default()
        },
    ));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn position_window() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::PositionWindowRequest(PositionWindowRequest::default()));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn drag_window() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::DragWindowRequest(DragWindowRequest {}));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn onboarding() {
    let mut sandbox = Sandbox::new();
    sandbox.send(Submessage::OnboardingRequest(OnboardingRequest {
        action: OnboardingAction::FinishOnboarding.into(),
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn read_file() {
    let mut sandbox = Sandbox::new();
    fs::write(sandbox.cwd().join("read.txt"), "hello").unwrap();
    fs::create_dir(sandbox.cwd().join("sub")).unwrap();
    fs::write(sandbox.cwd().join("sub/nested.txt"), "nested").unwrap();
    fs::write(sandbox.home().join("tilde.txt"), "from home").unwrap();

    sandbox.send(Submessage::ReadFileRequest(ReadFileRequest {
        path: file_path("read.txt"),
        is_binary_file: None,
    }));
    sandbox.send(Submessage::ReadFileRequest(ReadFileRequest {
        path: file_path("read.txt"),
        is_binary_file: Some(true),
    }));
    sandbox.send(Submessage::ReadFileRequest(ReadFileRequest {
        path: Some(FilePath {
            path: "nested.txt".into(),
            relative_to: Some("sub".into()),
            expand_tilde_in_path: None,
        }),
        is_binary_file: None,
    }));
    sandbox.send(Submessage::ReadFileRequest(ReadFileRequest {
        path: Some(FilePath {
            path: "~/tilde.txt".into(),
            relative_to: None,
            expand_tilde_in_path: Some(true),
        }),
        is_binary_file: None,
    }));
    sandbox.send(Submessage::ReadFileRequest(ReadFileRequest {
        path: file_path("missing.txt"),
        is_binary_file: None,
    }));
    sandbox.send(Submessage::ReadFileRequest(ReadFileRequest {
        path: None,
        is_binary_file: None,
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn write_file() {
    let mut sandbox = Sandbox::new();

    sandbox.send(Submessage::WriteFileRequest(WriteFileRequest {
        path: file_path("text.txt"),
        data: Some(write_file_request::Data::Text("text".into())),
    }));
    sandbox.send(Submessage::WriteFileRequest(WriteFileRequest {
        path: Some(FilePath {
            path: "~/binary.bin".into(),
            relative_to: None,
            expand_tilde_in_path: Some(true),
        }),
        data: Some(write_file_request::Data::Binary(b"binary".to_vec())),
    }));
    sandbox.send(Submessage::WriteFileRequest(WriteFileRequest {
        path: file_path("no-such-dir/file.txt"),
        data: Some(write_file_request::Data::Text("lost".into())),
    }));
    sandbox.send(Submessage::WriteFileRequest(WriteFileRequest {
        path: None,
        data: None,
    }));

    let (text, binary) = (sandbox.cwd().join("text.txt"), sandbox.home().join("binary.bin"));
    sandbox.note_file(&text);
    sandbox.note_file(&binary);
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn append_to_file() {
    let mut sandbox = Sandbox::new();
    fs::write(sandbox.cwd().join("log.txt"), "a").unwrap();

    sandbox.send(Submessage::AppendToFileRequest(AppendToFileRequest {
        path: file_path("log.txt"),
        data: Some(append_to_file_request::Data::Text("b".into())),
    }));
    sandbox.send(Submessage::AppendToFileRequest(AppendToFileRequest {
        path: file_path("log.txt"),
        data: Some(append_to_file_request::Data::Binary(b"c".to_vec())),
    }));
    sandbox.send(Submessage::AppendToFileRequest(AppendToFileRequest {
        path: file_path("missing.txt"),
        data: Some(append_to_file_request::Data::Text("lost".into())),
    }));

    let log = sandbox.cwd().join("log.txt");
    sandbox.note_file(&log);
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn destination_of_symbolic_link() {
    let mut sandbox = Sandbox::new();
    // `/etc` is a symlink to `/private/etc` on every macOS install, so the answer is stable
    // without putting the temp directory's name in the snapshot.
    sandbox.send(Submessage::DestinationOfSymbolicLinkRequest(
        DestinationOfSymbolicLinkRequest {
            path: file_path("/etc"),
        },
    ));
    sandbox.send(Submessage::DestinationOfSymbolicLinkRequest(
        DestinationOfSymbolicLinkRequest {
            path: file_path("missing-link"),
        },
    ));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn contents_of_directory() {
    let mut sandbox = Sandbox::new();
    fs::create_dir(sandbox.cwd().join("dir")).unwrap();
    // A single entry, so the order `read_dir` returns cannot matter.
    fs::write(sandbox.cwd().join("dir/only.txt"), "").unwrap();

    sandbox.send(Submessage::ContentsOfDirectoryRequest(ContentsOfDirectoryRequest {
        directory: file_path("dir"),
    }));
    sandbox.send(Submessage::ContentsOfDirectoryRequest(ContentsOfDirectoryRequest {
        directory: file_path("missing-dir"),
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn create_directory() {
    let mut sandbox = Sandbox::new();

    sandbox.send(Submessage::CreateDirectoryRequest(CreateDirectoryRequest {
        path: file_path("deep/a/b"),
        recursive: Some(true),
    }));
    sandbox.send(Submessage::CreateDirectoryRequest(CreateDirectoryRequest {
        path: file_path("flat"),
        recursive: Some(false),
    }));
    sandbox.send(Submessage::CreateDirectoryRequest(CreateDirectoryRequest {
        path: file_path("flat"),
        recursive: None,
    }));
    sandbox.send(Submessage::CreateDirectoryRequest(CreateDirectoryRequest {
        path: file_path("no-parent/child"),
        recursive: None,
    }));
    sandbox.send(Submessage::CreateDirectoryRequest(CreateDirectoryRequest {
        path: Some(FilePath {
            path: "~/made-in-home".into(),
            relative_to: None,
            expand_tilde_in_path: Some(true),
        }),
        recursive: None,
    }));

    let dirs = [
        sandbox.cwd().join("deep/a/b"),
        sandbox.cwd().join("flat"),
        sandbox.cwd().join("no-parent"),
        sandbox.home().join("made-in-home"),
    ];
    for dir in &dirs {
        sandbox.note_dir(dir);
    }
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn local_state() {
    let mut sandbox = Sandbox::new();
    let key = || Some("snapshot.key".to_owned());

    sandbox.send(Submessage::GetLocalStateRequest(GetLocalStateRequest { key: key() }));
    sandbox.send(Submessage::UpdateLocalStateRequest(UpdateLocalStateRequest {
        key: key(),
        value: Some(r#"{"nested":[1,2]}"#.into()),
    }));
    sandbox.send(Submessage::GetLocalStateRequest(GetLocalStateRequest { key: key() }));
    sandbox.send(Submessage::UpdateLocalStateRequest(UpdateLocalStateRequest {
        key: key(),
        value: Some("not json".into()),
    }));
    sandbox.send(Submessage::GetLocalStateRequest(GetLocalStateRequest { key: key() }));
    sandbox.send(Submessage::UpdateLocalStateRequest(UpdateLocalStateRequest {
        key: key(),
        value: None,
    }));
    sandbox.send(Submessage::GetLocalStateRequest(GetLocalStateRequest { key: key() }));
    sandbox.send(Submessage::UpdateLocalStateRequest(UpdateLocalStateRequest {
        key: None,
        value: Some("1".into()),
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn settings_property() {
    let mut sandbox = Sandbox::new();
    let key = || Some("snapshot.key".to_owned());

    sandbox.send(Submessage::GetSettingsPropertyRequest(GetSettingsPropertyRequest {
        key: key(),
    }));
    sandbox.send(Submessage::UpdateSettingsPropertyRequest(
        UpdateSettingsPropertyRequest {
            key: key(),
            value: Some("true".into()),
        },
    ));
    sandbox.send(Submessage::GetSettingsPropertyRequest(GetSettingsPropertyRequest {
        key: key(),
    }));
    sandbox.send(Submessage::GetSettingsPropertyRequest(GetSettingsPropertyRequest {
        key: None,
    }));
    sandbox.send(Submessage::UpdateSettingsPropertyRequest(
        UpdateSettingsPropertyRequest {
            key: key(),
            value: None,
        },
    ));
    sandbox.send(Submessage::GetSettingsPropertyRequest(GetSettingsPropertyRequest {
        key: key(),
    }));
    sandbox.send(Submessage::UpdateSettingsPropertyRequest(
        UpdateSettingsPropertyRequest {
            key: None,
            value: Some("1".into()),
        },
    ));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn install() {
    let mut sandbox = Sandbox::new();
    // Only requests that do not change the machine: a dotfiles status check against the empty
    // sandbox `HOME`, and a component macOS does not have. Accessibility is left out because its
    // status is the real TCC grant of whichever binary runs the test.
    sandbox.send(Submessage::InstallRequest(InstallRequest {
        component: InstallComponent::Dotfiles.into(),
        action: InstallAction::Status.into(),
    }));
    sandbox.send(Submessage::InstallRequest(InstallRequest {
        component: InstallComponent::Ibus.into(),
        action: InstallAction::Install.into(),
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn history_query() {
    let mut sandbox = Sandbox::new();

    sandbox.send(Submessage::HistoryQueryRequest(HistoryQueryRequest {
        query: "SELECT count(*) AS n, ?1 AS text, ?2 AS int, ?3 AS absent FROM history".into(),
        params: vec![
            Param {
                r#type: Some(ParamType::String("s".into())),
            },
            Param {
                r#type: Some(ParamType::Integer(7)),
            },
            Param {
                r#type: Some(ParamType::Null(())),
            },
        ],
    }));
    sandbox.send(Submessage::HistoryQueryRequest(HistoryQueryRequest {
        query: "DELETE FROM history".into(),
        params: vec![],
    }));
    sandbox.send(Submessage::HistoryQueryRequest(HistoryQueryRequest {
        query: "SELECT ?1".into(),
        params: vec![Param { r#type: None }],
    }));
    insta::assert_snapshot!(sandbox.transcript());
}

#[test]
fn open_in_external_application() {
    let mut sandbox = Sandbox::new();
    // Never a real URL: that would open a browser.
    sandbox.send(Submessage::OpenInExternalApplicationRequest(
        OpenInExternalApplicationRequest { url: None },
    ));
    insta::assert_snapshot!(sandbox.transcript());
}
