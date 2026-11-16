use std::collections::HashMap;
use std::time::{
    Duration,
    Instant,
};

use fig_proto::fig::RunProcessRequest;
use fig_proto::fig::server_originated_message::Submessage;
use fig_shell_ipc::process::{
    self,
    Request,
    Shell,
};
use fig_shell_ipc::session::ShellSessions;
use parking_lot::Mutex;
use tokio::sync::watch;
use uuid::Uuid;

use super::RequestResult;
use crate::webview::WindowId;

type Key = (WindowId, Uuid);
struct Entry {
    cancel: watch::Sender<bool>,
    active: bool,
    updated: Instant,
}

/// Bounded cancellation ownership. Tombstones handle cancellation arriving before a request
/// on the independently scheduled custom-protocol tasks, and reject token reuse.
#[derive(Default)]
pub struct Processes {
    entries: Mutex<HashMap<Key, Entry>>,
}
impl Processes {
    pub fn cancel_window(&self, window: &WindowId) {
        for ((owner, _), entry) in self.entries.lock().iter_mut() {
            if owner == window {
                entry.cancel.send_replace(true);
                entry.updated = Instant::now();
            }
        }
    }

    fn prune(entries: &mut HashMap<Key, Entry>) {
        entries.retain(|_, e| e.active || e.updated.elapsed() < Duration::from_secs(70));
    }

    pub fn cancel(&self, window: WindowId, token: &str) -> Result<(), String> {
        let key = (window, Uuid::parse_str(token).map_err(|e| e.to_string())?);
        let mut entries = self.entries.lock();
        Self::prune(&mut entries);
        if let Some(entry) = entries.get_mut(&key) {
            entry.cancel.send_replace(true);
            entry.updated = Instant::now();
        } else {
            if entries.len() >= 4096 {
                return Err("Process registry is full".into());
            }
            entries.insert(key, Entry {
                cancel: watch::channel(true).0,
                active: false,
                updated: Instant::now(),
            });
        }
        Ok(())
    }

    fn register(&self, key: Key) -> Result<(Registration<'_>, watch::Receiver<bool>), String> {
        let mut entries = self.entries.lock();
        Self::prune(&mut entries);
        if entries.contains_key(&key) {
            return Err("Process cancelled or token already used".into());
        }
        if entries.len() >= 4096 {
            return Err("Process registry is full".into());
        }
        let (cancel, receiver) = watch::channel(false);
        entries.insert(key.clone(), Entry {
            cancel,
            active: true,
            updated: Instant::now(),
        });
        Ok((Registration { owner: self, key }, receiver))
    }
}
struct Registration<'a> {
    owner: &'a Processes,
    key: Key,
}
impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let mut entries = self.owner.entries.lock();
        if let Some(entry) = entries.get_mut(&self.key) {
            if *entry.cancel.borrow() {
                entry.active = false;
                entry.updated = Instant::now();
            } else {
                entries.remove(&self.key);
            }
        }
    }
}

pub async fn run(
    request: RunProcessRequest,
    window: WindowId,
    processes: &Processes,
    shell_sessions: &ShellSessions,
) -> RequestResult {
    let token = request
        .cancellation_token
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|err| format!("invalid cancellation token: {err}"))?
        .unwrap_or_else(Uuid::new_v4);
    let (_registration, mut cancelled) = processes.register((window, token))?;
    let uuid = request
        .terminal_session_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|err| format!("terminal_session_id is not a valid UUID: {err}"))?;
    let shell = shell_sessions.with_maybe_id(&uuid, |session| Shell {
        env: session
            .context
            .as_ref()
            .map_or_else(Vec::new, |c| c.environment_variables.clone()),
        cwd: session
            .context
            .as_ref()
            .and_then(|c| c.current_working_directory.clone()),
    });
    if uuid.is_some() && shell.is_none() {
        return Err("Terminal session has disconnected".into());
    }
    let run = process::run(
        Request {
            executable: request.executable,
            arguments: request.arguments,
            working_directory: request.working_directory,
            env: request.env,
            timeout: request.timeout.map(Into::into),
        },
        shell.unwrap_or(Shell { env: vec![], cwd: None }),
    );
    let response = tokio::select! {
        biased;
        _ = cancelled.wait_for(|value| *value) => return Err("Process cancelled".into()),
        response = run => response?,
    };
    Ok(Box::new(Submessage::RunProcessResponse(response)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AUTOCOMPLETE_ID,
        DASHBOARD_ID,
    };
    #[tokio::test]
    async fn cancellation_is_scoped_and_handles_reordered_requests() {
        let registry = Processes::default();
        let token = Uuid::new_v4();
        registry.cancel(AUTOCOMPLETE_ID, &token.to_string()).unwrap();
        assert!(registry.register((AUTOCOMPLETE_ID, token)).is_err());
        let (_guard, mut receiver) = registry.register((DASHBOARD_ID, token)).unwrap();
        assert!(!*receiver.borrow());
        registry.cancel(DASHBOARD_ID, &token.to_string()).unwrap();
        receiver.wait_for(|value| *value).await.unwrap();
    }
    #[test]
    fn cancellation_registry_has_a_hard_bound() {
        let registry = Processes::default();
        for _ in 0..4096 {
            registry.cancel(AUTOCOMPLETE_ID, &Uuid::new_v4().to_string()).unwrap();
        }
        assert!(registry.cancel(AUTOCOMPLETE_ID, &Uuid::new_v4().to_string()).is_err());
    }
}
