use thiserror::Error;

use crate::local::*;
use crate::util::get_shell;

type Result<T, E = HookError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum HookError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    GetShell(#[from] crate::util::GetShellError),
}

fn hook_enum_to_hook(hook: hook::Hook) -> Hook {
    Hook { hook: Some(hook) }
}

pub fn hook_to_message(hook: Hook) -> LocalMessage {
    LocalMessage {
        r#type: Some(local_message::Type::Hook(hook)),
    }
}

pub fn generate_shell_context(
    pid: impl Into<i32>,
    tty: impl Into<String>,
    session_id: Option<impl Into<String>>,
) -> Result<ShellContext> {
    let cwd = std::env::current_dir()?;
    let shell = get_shell()?;
    Ok(ShellContext {
        pid: Some(pid.into()),
        ttys: Some(tty.into()),
        session_id: session_id.map(|s| s.into()),
        process_name: Some(shell),
        current_working_directory: Some(cwd.to_string_lossy().into()),
        ..Default::default()
    })
}

pub fn new_event_hook(
    event_name: impl Into<String>,
    payload: impl Into<Option<String>>,
    apps: impl Into<Vec<String>>,
) -> Hook {
    hook_enum_to_hook(hook::Hook::Event(EventHook {
        event_name: event_name.into(),
        payload: payload.into(),
        apps: apps.into(),
    }))
}

pub fn new_clear_autocomplete_cache(clis: Vec<String>) -> Hook {
    hook_enum_to_hook(hook::Hook::ClearAutocompleteCache(ClearAutocompleteCacheHook { clis }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_hooks() {
        let _ = generate_shell_context(1, "tty", Some("session_id")).unwrap();
        let _ = new_event_hook("event_name", Some("payload".into()), vec!["app".into()]);
        let _ = new_clear_autocomplete_cache(vec!["cli".into()]);
    }
}
