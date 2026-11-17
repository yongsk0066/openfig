use fig_proto::fig::InsertTextRequest;
use fig_shell_ipc::session::{
    ShellCommand,
    ShellSessions,
};
use uuid::Uuid;

use super::{
    RequestResult,
    RequestResultImpl,
};

pub async fn insert_text(
    request: InsertTextRequest,
    state: &ShellSessions,
    proxy: &dyn crate::event::EventSender,
) -> RequestResult {
    let mut acknowledgement = None;
    let command = match request.r#type {
        Some(some) => match some {
            fig_proto::fig::insert_text_request::Type::Text(text) => ShellCommand::InsertText {
                insertion: Some(text),
                deletion: None,
                immediate: None,
                offset: None,
                insertion_buffer: None,
                insert_during_command: None,
            },
            fig_proto::fig::insert_text_request::Type::Update(update) if update.input_revision.is_some() => {
                let (response, receiver) = tokio::sync::oneshot::channel();
                acknowledgement = Some(receiver);
                ShellCommand::ApplyEdit {
                    expires_at: std::time::Instant::now() + std::time::Duration::from_millis(1500),
                    revision: update.input_revision.ok_or("input_revision is required")?,
                    cursor_utf16: update.expected_cursor.ok_or("expected_cursor is required")?,
                    buffer: update.insertion_buffer.ok_or("insertion_buffer is required")?,
                    edit: fig_shell_ipc::hook::wire::Insert {
                        delete_before: usize::try_from(update.deletion.unwrap_or(0))
                            .map_err(|_error| "invalid deletion")?,
                        text: update.insertion.unwrap_or_default(),
                        cursor_left: isize::try_from(update.offset.unwrap_or(0).checked_neg().ok_or("invalid offset")?)
                            .map_err(|_error| "invalid offset")?,
                        accept: update.immediate.unwrap_or(false),
                    },
                    response,
                }
            },
            fig_proto::fig::insert_text_request::Type::Update(update) => ShellCommand::InsertText {
                insertion: update.insertion,
                deletion: update.deletion,
                immediate: update.immediate,
                offset: update.offset,
                insertion_buffer: update.insertion_buffer,
                insert_during_command: None,
            },
        },
        None => return RequestResult::error("InsertTextRequest expects a request type"),
    };

    let uuid_str = request.terminal_session_id.ok_or("terminal_session_id is required")?;
    let uuid = Uuid::parse_str(&uuid_str).map_err(|err| format!("terminal_session_id is not a valid UUID: {err}"))?;

    match state.with(&uuid, |session| session.sender.clone()) {
        Some(sender) => {
            let mut sent = None;
            proxy.with_edit_buffer_barrier(Box::new(|| {
                sent = Some(sender.send(command));
            }));
            sent.ok_or("edit barrier did not dispatch the shell command")?
                .map_err(|err| format!("Failed sending command to shell session: {err}"))?;
            if let Some(receiver) = acknowledgement {
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver)
                    .await
                    .map_err(|_error| "shell edit outcome unknown: acknowledgement timed out; not retried")?
                    .map_err(|_error| "shell edit outcome unknown: disconnected before acknowledgement")??;
            }
            RequestResult::success()
        },
        None => RequestResult::error("No shell sessions"),
    }
}
