use fig_proto::fig::UpdateApplicationPropertiesRequest;
use fig_settings::keybindings::{
    KeyBinding,
    KeyBindings,
};
use fig_shell_ipc::session::{
    InterceptMode,
    ShellCommand,
    ShellSessions,
};
use tracing::error;
use uuid::Uuid;

use super::{
    RequestResult,
    RequestResultImpl,
};
use crate::InterceptState;

pub fn update(
    request: UpdateApplicationPropertiesRequest,
    shell_sessions: &ShellSessions,
    intercept_state: &InterceptState,
) -> RequestResult {
    if let Some(intercept_bound_keystrokes) = request.intercept_bound_keystrokes {
        *intercept_state.intercept_bound_keystrokes.write().unwrap() = intercept_bound_keystrokes;
    }

    if let Some(intercept_global_keystrokes) = request.intercept_global_keystrokes {
        *intercept_state.intercept_global_keystrokes.write().unwrap() = intercept_global_keystrokes;
    }

    let key_bindings = KeyBindings::load_from_settings("autocomplete")
        .map_or_else(
            |err| {
                error!(%err, "Failed to load keybindings");
                vec![].into_iter()
            },
            |key_bindings| key_bindings.into_iter(),
        )
        .map(|KeyBinding { identifier, binding }| fig_proto::shell::Action {
            identifier,
            bindings: vec![binding],
        });

    let actions = request
        .action_list
        .into_iter()
        .flat_map(|list| {
            list.actions.into_iter().filter_map(|action| {
                action.identifier.map(|identifier| fig_proto::shell::Action {
                    identifier,
                    bindings: action.default_bindings,
                })
            })
        })
        .chain(key_bindings)
        .collect::<Vec<_>>();

    let request_session_id = request
        .current_terminal_session_id
        .and_then(|i| Uuid::parse_str(&i).ok());

    for session in shell_sessions.inner.lock().linked_sessions.values_mut() {
        if request_session_id.as_ref() == Some(&session.id) {
            session.intercept = request.intercept_bound_keystrokes.unwrap_or_default().into();
            session.intercept_global = request.intercept_global_keystrokes.unwrap_or_default().into();

            if let Err(err) = session.sender.send(ShellCommand::InterceptFigJs {
                intercept_keystrokes: request.intercept_bound_keystrokes.unwrap_or_default(),
                intercept_global_keystrokes: request.intercept_global_keystrokes.unwrap_or_default(),
                actions: actions.clone(),
                override_actions: true,
            }) {
                error!(%err, %session.id, "Failed sending command to shell session");
            }
        } else {
            session.intercept = InterceptMode::Unlocked;

            if let Err(err) = session.sender.send(ShellCommand::InterceptFigJs {
                intercept_keystrokes: false,
                intercept_global_keystrokes: request.intercept_global_keystrokes.unwrap_or_default(),
                actions: vec![],
                override_actions: false,
            }) {
                error!(%err, %session.id, "Failed sending command to shell session");
            }
        }
    }

    RequestResult::success()
}
