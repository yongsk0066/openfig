use std::collections::HashMap;
use std::time::Duration;

use fig_proto::fig::{
    EnvironmentVariable,
    RunProcessResponse,
};
use fig_proto::local::{
    ShellContext,
    TerminalCursorCoordinates,
};
use parking_lot::lock_api::MutexGuard;
use parking_lot::{
    FairMutex,
    MappedFairMutexGuard,
    RawFairMutex,
};
use serde::Serialize;
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Clone, Default, Debug)]
pub struct EditBuffer {
    pub text: String,
    pub cursor: i64,
}

#[derive(Debug, Default, Serialize)]
struct InnerShellSessions {
    /// Every connected shell, by session id.
    linked_sessions: HashMap<Uuid, ShellSession>,
    /// The session that reported last.
    most_recent: Option<Uuid>,
}

/// The shells connected to the desktop, each a `fig.zsh` on `hook.sock` ([`crate::hook`]).
#[derive(Debug, Default, Serialize)]
pub struct ShellSessions {
    #[serde(flatten)]
    inner: FairMutex<InnerShellSessions>,
}

impl ShellSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Copy only the values a caller needs; no session guard escapes this synchronous scope.
    pub fn collect<T>(&self, mut select: impl FnMut(&mut ShellSession) -> T) -> Vec<T> {
        self.inner
            .lock()
            .linked_sessions
            .values_mut()
            .map(&mut select)
            .collect()
    }

    /// Sending happens after releasing the registry lock. A full queue may disconnect a shell.
    pub fn broadcast(&self, command: impl Fn() -> ShellCommand) {
        let senders = self.collect(|session| session.sender.clone());
        for sender in senders {
            let _ = sender.send(command());
        }
    }

    /// Inserts a new session id
    pub(crate) fn insert(&self, session: ShellSession) {
        let mut sessions = self.inner.lock();
        sessions.most_recent = Some(session.id);
        sessions.linked_sessions.insert(session.id, session);
    }

    /// Gets mutable reference to the given session id and sets the most recent session id
    pub fn with_update<T>(&self, key: Uuid, f: impl FnOnce(&mut ShellSession) -> T) -> Option<T> {
        let mut guard = self.inner.lock();
        let res = guard.linked_sessions.get_mut(&key).map(f);

        if res.is_some() {
            guard.most_recent = Some(key);
        }

        res
    }

    fn with_most_recent<T>(&self, f: impl FnOnce(&mut ShellSession) -> T) -> Option<T> {
        let mut guard = self.inner.lock();
        let id = guard.most_recent?;
        guard.linked_sessions.get_mut(&id).map(f)
    }

    /// Gets mutable reference to the given session id
    pub fn with<T>(&self, session_id: &Uuid, f: impl FnOnce(&mut ShellSession) -> T) -> Option<T> {
        let mut guard = self.inner.lock();
        guard.linked_sessions.get_mut(session_id).map(f)
    }

    pub fn most_recent(&self) -> Option<MappedFairMutexGuard<'_, ShellSession>> {
        MutexGuard::<'_, RawFairMutex, InnerShellSessions>::try_map(
            self.inner.lock(),
            |guard: &mut InnerShellSessions| {
                guard
                    .most_recent
                    .as_mut()
                    .and_then(|id| guard.linked_sessions.get_mut(id))
            },
        )
        .ok()
    }

    pub fn with_maybe_id<T>(&self, session_id: &Option<Uuid>, f: impl FnOnce(&mut ShellSession) -> T) -> Option<T> {
        match session_id {
            Some(session_id) => self.with(session_id, f),
            None => self.with_most_recent(f),
        }
    }

    pub(crate) fn remove_id(&self, session_id: &Uuid) -> Option<ShellSession> {
        let mut guard = self.inner.lock();
        if guard.most_recent.as_ref() == Some(session_id) {
            guard.most_recent = None;
        }
        guard.linked_sessions.remove(session_id)
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum InterceptMode {
    Locked,
    Unlocked,
}

impl From<bool> for InterceptMode {
    fn from(from: bool) -> Self {
        if from {
            InterceptMode::Locked
        } else {
            InterceptMode::Unlocked
        }
    }
}

impl From<InterceptMode> for bool {
    fn from(from: InterceptMode) -> Self {
        match from {
            InterceptMode::Locked => true,
            InterceptMode::Unlocked => false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ShellSession {
    pub id: Uuid,
    #[serde(skip)]
    pub sender: ShellSender,
    #[serde(skip)]
    pub edit_buffer: EditBuffer,
    pub context: Option<ShellContext>,
    #[serde(skip)]
    pub terminal_cursor_coordinates: Option<TerminalCursorCoordinates>,
    pub intercept: InterceptMode,
    pub intercept_global: InterceptMode,
}

/// What the desktop asks of a shell session; [`crate::hook`] carries it out.
#[derive(Debug)]
pub enum ShellCommand {
    ApplyEdit {
        expires_at: std::time::Instant,
        revision: u64,
        cursor_utf16: u64,
        buffer: String,
        edit: crate::hook::wire::Insert,
        response: oneshot::Sender<Result<(), String>>,
    },
    InterceptFigJs {
        intercept_keystrokes: bool,
        intercept_global_keystrokes: bool,
        actions: Vec<fig_proto::shell::Action>,
        override_actions: bool,
    },
    InterceptFigJSVisible {
        visible: bool,
    },
    InsertText {
        insertion: Option<String>,
        deletion: Option<i64>,
        offset: Option<i64>,
        immediate: Option<bool>,
        insertion_buffer: Option<String>,
        insert_during_command: Option<bool>,
    },
    RunProcess {
        channel: oneshot::Sender<Result<RunProcessResponse, String>>,
        executable: String,
        arguments: Vec<String>,
        working_directory: Option<String>,
        env: Vec<EnvironmentVariable>,
        timeout: Option<Duration>,
    },
}

impl ShellCommand {
    /// [`ShellCommand::RunProcess`] and the receiver its answer arrives on.
    pub fn run_process(
        executable: String,
        arguments: Vec<String>,
        working_directory: Option<String>,
        env: Vec<EnvironmentVariable>,
        timeout: Option<Duration>,
    ) -> (Self, oneshot::Receiver<Result<RunProcessResponse, String>>) {
        let (channel, rx) = oneshot::channel();
        (
            Self::RunProcess {
                channel,
                executable,
                arguments,
                working_directory,
                env,
                timeout,
            },
            rx,
        )
    }
}

/// A UI thread must never block on a saturated shell. Overflow closes the hook connection,
/// letting zsh restore its normal widgets instead of silently losing an insertion or key.
#[derive(Debug, Clone)]
pub struct ShellSender {
    sender: flume::Sender<ShellCommand>,
    disconnect: tokio::sync::watch::Sender<bool>,
}
impl ShellSender {
    pub fn new(sender: flume::Sender<ShellCommand>, disconnect: tokio::sync::watch::Sender<bool>) -> Self {
        Self { sender, disconnect }
    }

    pub fn send(&self, command: ShellCommand) -> Result<(), flume::SendError<ShellCommand>> {
        self.sender.try_send(command).map_err(|error| {
            self.disconnect.send_replace(true);
            flume::SendError(error.into_inner())
        })
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn overload_disconnects_instead_of_blocking_or_silently_dropping() {
        let (tx, rx) = flume::bounded(1);
        let (close, receiver) = tokio::sync::watch::channel(false);
        let tx = ShellSender::new(tx, close);
        tx.send(ShellCommand::InterceptFigJSVisible { visible: true }).unwrap();
        assert!(tx.send(ShellCommand::InterceptFigJSVisible { visible: false }).is_err());
        assert!(*receiver.borrow());
        assert!(matches!(rx.recv().unwrap(), ShellCommand::InterceptFigJSVisible {
            visible: true
        }));
    }
}
