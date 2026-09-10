use std::sync::Arc;

use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
};
use figterm::FigtermState;
use uuid::Uuid;

pub mod figterm;
pub mod remote;

/// Handlers for the hooks figterm pushes over the remote socket.
///
/// These used to return `Option<clientbound::response::Response>`, but the only response variant
/// the proto ever had was `Empty` and every implementation returned `Ok(None)`, so figterm never
/// received one — and had no branch to receive it with. The whole `Clientbound.Response` message
/// is gone; the hooks are one-way.
#[async_trait::async_trait]
pub trait RemoteHookHandler {
    type Error: std::fmt::Display;

    async fn edit_buffer(
        &mut self,
        edit_buffer_hook: &EditBufferHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error>;

    async fn prompt(
        &mut self,
        prompt_hook: &PromptHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error>;

    async fn pre_exec(
        &mut self,
        pre_exec_hook: &PreExecHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error>;

    async fn post_exec(
        &mut self,
        post_exec_hook: &PostExecHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error>;

    async fn intercepted_key(
        &mut self,
        intercepted_key: InterceptedKeyHook,
        session_id: Uuid,
    ) -> Result<(), Self::Error>;

    /// This is not technically a hook, it is triggers by many other hooks and does not allow for a
    /// response, mostly used for diagnostics and testing
    async fn shell_context(&mut self, _context: &ShellContext, _session_id: Uuid) {}
}
