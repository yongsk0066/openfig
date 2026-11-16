//! Coalesce only adjacent edit-buffer deliveries. Every other event is an ordering barrier.
use std::sync::{
    Arc,
    Weak,
};

use fig_proto::fig::EditBufferChangedNotification;
use parking_lot::Mutex;
use tao::event_loop::{
    EventLoopClosed,
    EventLoopProxy as TaoProxy,
};
use uuid::Uuid;

use crate::event::Event;
use crate::webview::WindowId;

#[derive(Debug)]
pub struct EditDelivery {
    pub window: WindowId,
    pub channel: i64,
    pub generation: u64,
    pub notification: EditBufferChangedNotification,
}

#[derive(Debug)]
pub struct EditBufferBatch {
    pub received: std::time::Instant,
    pub session: Uuid,
    pub empty: bool,
    pub deliveries: Vec<EditDelivery>,
}

impl EditBufferBatch {
    fn can_merge(&self, next: &Self) -> bool {
        self.session == next.session
            && self.deliveries.len() == next.deliveries.len()
            && self.deliveries.iter().zip(&next.deliveries).all(|(a, b)| {
                a.window == b.window
                    && a.channel == b.channel
                    && a.generation == b.generation
                    && a.notification.context.is_some() == b.notification.context.is_some()
            })
    }

    fn merge(&mut self, mut next: Self) {
        for (old, new) in self.deliveries.iter_mut().zip(&mut next.deliveries) {
            // Preserve a pending full snapshot when the next keystroke is only a delta.
            // A new full snapshot, including an empty one, supersedes the old snapshot.
            if let Some(context) = new.notification.context.as_mut()
                && context.context_snapshot != Some(true)
                && let Some(previous) = old.notification.context.as_mut()
                && previous.context_snapshot == Some(true)
            {
                context.environment_variables = std::mem::take(&mut previous.environment_variables);
                context.alias = previous.alias.take();
                context.context_snapshot = Some(true);
            }
        }
        *self = next;
    }
}

pub type PendingEdit = Arc<Mutex<Option<EditBufferBatch>>>;

#[derive(Debug, Default)]
struct Tail {
    pending: Weak<Mutex<Option<EditBufferBatch>>>,
}
impl Tail {
    fn barrier(&mut self) {
        self.pending = Weak::new();
    }

    fn push(&mut self, next: EditBufferBatch) -> Option<PendingEdit> {
        if let Some(pending) = self.pending.upgrade() {
            let mut current = pending.lock();
            if let Some(current) = current.as_mut()
                && current.can_merge(&next)
            {
                current.merge(next);
                return None;
            }
        }
        let pending = Arc::new(Mutex::new(Some(next)));
        self.pending = Arc::downgrade(&pending);
        Some(pending)
    }
}

#[derive(Debug, Clone)]
pub struct EventLoopProxy {
    inner: TaoProxy<Event>,
    tail: Arc<Mutex<Tail>>,
}
impl EventLoopProxy {
    pub fn new(inner: TaoProxy<Event>) -> Self {
        Self {
            inner,
            tail: Arc::default(),
        }
    }

    pub fn with_edit_barrier(&self, action: impl FnOnce()) {
        let mut tail = self.tail.lock();
        tail.barrier();
        action();
    }

    pub fn send_event(&self, event: Event) -> Result<(), Box<EventLoopClosed<Event>>> {
        // Keep barrier invalidation and enqueue in one critical section across all producers.
        let mut tail = self.tail.lock();
        tail.barrier();
        self.inner.send_event(event).map_err(Box::new)
    }

    pub fn send_edit_buffer(&self, batch: EditBufferBatch) -> Result<(), Box<EventLoopClosed<Event>>> {
        let _enqueue = crate::latency::Measure::new("input-enqueue");
        let mut tail = {
            let _lock = crate::latency::Measure::new("input-queue-lock");
            self.tail.lock()
        };
        if let Some(pending) = tail.push(batch) {
            self.inner.send_event(Event::EditBuffer(pending)).map_err(Box::new)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use fig_proto::fig::EnvironmentVariable;
    use fig_proto::local::ShellContext;

    use super::*;
    fn batch(session: Uuid, text: &str, full: bool) -> EditBufferBatch {
        EditBufferBatch {
            received: std::time::Instant::now(),
            session,
            empty: text.is_empty(),
            deliveries: vec![EditDelivery {
                window: crate::AUTOCOMPLETE_ID,
                channel: 7,
                generation: 1,
                notification: EditBufferChangedNotification {
                    buffer: Some(text.into()),
                    cursor: Some(text.encode_utf16().count() as i32),
                    session_id: Some(session.to_string()),
                    context: Some(ShellContext {
                        context_snapshot: Some(full),
                        environment_variables: if full {
                            vec![EnvironmentVariable {
                                key: "PATH".into(),
                                value: Some("/bin".into()),
                            }]
                        } else {
                            vec![]
                        },
                        alias: full.then(|| "gs=git status".into()),
                        ..Default::default()
                    }),
                },
            }],
        }
    }
    #[test]
    fn burst_keeps_latest_cursor_and_first_full_snapshot() {
        let mut tail = Tail::default();
        let session = Uuid::new_v4();
        let pending = tail.push(batch(session, "g", true)).unwrap();
        for _ in 0..1000 {
            assert!(tail.push(batch(session, "git 🦀", false)).is_none());
        }
        let current = pending.lock().take().unwrap();
        assert_eq!(current.deliveries[0].notification.cursor, Some(6));
        assert_eq!(current.deliveries[0].notification.buffer.as_deref(), Some("git 🦀"));
        assert_eq!(
            current.deliveries[0]
                .notification
                .context
                .as_ref()
                .unwrap()
                .environment_variables
                .len(),
            1
        );
        assert!(
            tail.push(batch(session, "git status", false)).is_some(),
            "consumed wakeups cannot swallow a new edit"
        );
    }
    #[test]
    fn barriers_sessions_and_subscription_generations_prevent_reordering() {
        let mut tail = Tail::default();
        let session = Uuid::new_v4();
        let first = tail.push(batch(session, "before key", true)).unwrap();
        tail.barrier(); // KEY, INSERT, preexec, prompt, settings, focus, reload, etc.
        let second = tail.push(batch(session, "after key", false)).unwrap();
        assert_eq!(
            first.lock().as_ref().unwrap().deliveries[0]
                .notification
                .buffer
                .as_deref(),
            Some("before key")
        );
        assert!(tail.push(batch(Uuid::new_v4(), "another shell", true)).is_some());
        let mut reload = batch(session, "new page", true);
        reload.deliveries[0].generation = 2;
        assert!(tail.push(reload).is_some());
        assert_eq!(
            second.lock().as_ref().unwrap().deliveries[0]
                .notification
                .buffer
                .as_deref(),
            Some("after key")
        );
    }
    #[test]
    fn empty_full_snapshot_and_empty_buffer_override_previous_values() {
        let mut tail = Tail::default();
        let session = Uuid::new_v4();
        let pending = tail.push(batch(session, "old", true)).unwrap();
        let mut empty = batch(session, "", true);
        let context = empty.deliveries[0].notification.context.as_mut().unwrap();
        context.environment_variables.clear();
        context.alias = Some(String::new());
        assert!(tail.push(empty).is_none());
        let value = pending.lock().take().unwrap();
        assert!(value.empty);
        let context = value.deliveries[0].notification.context.as_ref().unwrap();
        assert!(context.environment_variables.is_empty());
        assert_eq!(context.alias.as_deref(), Some(""));
    }
}
