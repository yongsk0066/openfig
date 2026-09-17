//! The desktop's side of the shells: `fig.zsh` sessions on `hook.sock` ([`hook`]), the registry of
//! connected shells ([`session`]) and the handler the desktop reacts to their hooks with
//! ([`ShellHookHandler`]).

use std::sync::Arc;

use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
};
use session::ShellSessions;
use tracing::{
    error,
    trace,
};
use uuid::Uuid;

pub mod hook;
pub mod session;

/// What a shell session's hook tells the desktop. [`hook`] turns `fig.zsh`'s lines into these.
#[derive(Debug)]
pub enum HookRequest {
    EditBuffer(EditBufferHook),
    Prompt(PromptHook),
    PreExec(PreExecHook),
    PostExec(PostExecHook),
    InterceptedKey(InterceptedKeyHook),
}

/// Handlers for the hooks shell sessions report. The hooks are one-way: nothing is answered.
#[async_trait::async_trait]
pub trait ShellHookHandler {
    type Error: std::fmt::Display;

    async fn edit_buffer(
        &mut self,
        edit_buffer_hook: &EditBufferHook,
        session_id: Uuid,
        sessions: &Arc<ShellSessions>,
    ) -> Result<(), Self::Error>;

    async fn prompt(
        &mut self,
        prompt_hook: &PromptHook,
        session_id: Uuid,
        sessions: &Arc<ShellSessions>,
    ) -> Result<(), Self::Error>;

    async fn pre_exec(
        &mut self,
        pre_exec_hook: &PreExecHook,
        session_id: Uuid,
        sessions: &Arc<ShellSessions>,
    ) -> Result<(), Self::Error>;

    async fn post_exec(
        &mut self,
        post_exec_hook: &PostExecHook,
        session_id: Uuid,
        sessions: &Arc<ShellSessions>,
    ) -> Result<(), Self::Error>;

    async fn intercepted_key(
        &mut self,
        intercepted_key: InterceptedKeyHook,
        session_id: Uuid,
    ) -> Result<(), Self::Error>;

    /// This is not technically a hook, it is triggers by many other hooks and does not allow for a
    /// response, mostly used for diagnostics and testing
    async fn shell_context(&mut self, _context: &ShellContext, _session_id: Uuid) {}

    /// A command finished in a session. A shell hook has no process of its own to record history
    /// from, so the desktop does.
    async fn command_finished(&mut self, _command: fig_settings::history::CommandInfo) {}
}

/// Hands one hook of `session_id` to `handler`.
///
/// WARNING, when adding new hooks you must sanitize the context, otherwise the client can forge a
/// message from another session.
pub(crate) async fn dispatch_hook(
    request: HookRequest,
    session_id: Uuid,
    sessions: &Arc<ShellSessions>,
    handler: &mut (impl ShellHookHandler + Send),
) {
    // `scripts/latency-trace.sh` starts a keystroke's timeline at this line.
    trace!(?request, "Received shell hook");
    let res = match request {
        HookRequest::EditBuffer(mut edit_buffer) => {
            sanitize_fn(&mut edit_buffer.context, session_id);
            if let Some(shell_context) = &edit_buffer.context {
                handler.shell_context(shell_context, session_id).await;
            }
            handler.edit_buffer(&edit_buffer, session_id, sessions).await
        },
        HookRequest::Prompt(mut prompt) => {
            sanitize_fn(&mut prompt.context, session_id);
            if let Some(shell_context) = &prompt.context {
                handler.shell_context(shell_context, session_id).await;
            }
            handler.prompt(&prompt, session_id, sessions).await
        },
        HookRequest::PreExec(mut pre_exec) => {
            sanitize_fn(&mut pre_exec.context, session_id);
            if let Some(shell_context) = &pre_exec.context {
                handler.shell_context(shell_context, session_id).await;
            }
            handler.pre_exec(&pre_exec, session_id, sessions).await
        },
        HookRequest::PostExec(mut post_exec) => {
            sanitize_fn(&mut post_exec.context, session_id);
            if let Some(shell_context) = &post_exec.context {
                handler.shell_context(shell_context, session_id).await;
            }
            handler.post_exec(&post_exec, session_id, sessions).await
        },
        HookRequest::InterceptedKey(mut intercepted_key) => {
            sanitize_fn(&mut intercepted_key.context, session_id);
            if let Some(shell_context) = &intercepted_key.context {
                handler.shell_context(shell_context, session_id).await;
            }
            handler.intercepted_key(intercepted_key, session_id).await
        },
    };

    if let Err(err) = res {
        error!(%err, "Failed processing hook");
    }
}

// This has to be used to sanitize as a hook can contain an invalid session_id and it must
// be sanitized before being sent to any consumers
fn sanitize_fn(context: &mut Option<ShellContext>, session_id: Uuid) {
    if let Some(context) = context {
        context.session_id = Some(session_id.to_string());
    }
}
