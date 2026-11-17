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

pub fn update(request: UpdateApplicationPropertiesRequest, shell_sessions: &ShellSessions) -> RequestResult {
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

    let deliveries = shell_sessions.collect(|session| {
        let selected = request_session_id.as_ref() == Some(&session.id);
        session.intercept = if selected {
            request.intercept_bound_keystrokes.unwrap_or_default().into()
        } else {
            InterceptMode::Unlocked
        };
        session.intercept_global = request.intercept_global_keystrokes.unwrap_or_default().into();
        (session.id, session.sender.clone(), ShellCommand::InterceptFigJs {
            intercept_keystrokes: selected && request.intercept_bound_keystrokes.unwrap_or_default(),
            intercept_global_keystrokes: request.intercept_global_keystrokes.unwrap_or_default(),
            actions: if selected { actions.clone() } else { vec![] },
            override_actions: selected,
        })
    });
    for (id, sender, command) in deliveries {
        if let Err(err) = sender.send(command) {
            error!(%err,%id,"Failed sending command to shell session");
        }
    }

    RequestResult::success()
}
