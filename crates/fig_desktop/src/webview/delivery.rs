use std::collections::VecDeque;
use std::time::{
    Duration,
    Instant,
};

use fig_proto::fig::EditBufferChangedNotification;

pub enum Frame {
    Script(String),
    Edit {
        channel: i64,
        value: Box<EditBufferChangedNotification>,
    },
}
impl Frame {
    fn merge(&mut self, next: &mut Self) -> bool {
        if let (Self::Edit { channel: a, value: old }, Self::Edit { channel: b, value: new }) = (self, next)
            && a == b
            && old.session_id == new.session_id
            && old.context.is_some() == new.context.is_some()
        {
            if let (Some(previous), Some(context)) = (&mut old.context, &mut new.context)
                && previous.context_snapshot == Some(true)
                && context.context_snapshot != Some(true)
            {
                context.environment_variables = std::mem::take(&mut previous.environment_variables);
                context.alias = previous.alias.take();
                context.context_snapshot = Some(true);
            }
            *old = std::mem::take(new);
            return true;
        }
        false
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Ready,
    Recovering,
    Failed,
}
pub struct Delivery {
    pub phase: Phase,
    queue: VecDeque<Frame>,
    flight: Option<(u64, Instant)>,
    next_id: u64,
    started: Instant,
    failures: VecDeque<Instant>,
}
impl Default for Delivery {
    fn default() -> Self {
        Self {
            phase: Phase::Loading,
            queue: VecDeque::new(),
            flight: None,
            next_id: 0,
            started: Instant::now(),
            failures: VecDeque::new(),
        }
    }
}
impl Delivery {
    pub fn reset(&mut self) {
        self.queue.clear();
        self.flight = None;
        self.phase = Phase::Loading;
        self.started = Instant::now();
    }

    pub fn ready(&mut self) {
        self.phase = Phase::Ready;
    }

    pub fn push(&mut self, mut frame: Frame) -> bool {
        if self.phase != Phase::Ready {
            return true;
        } // the ready handshake replays authoritative state
        if self.queue.back_mut().is_some_and(|last| last.merge(&mut frame)) {
            return true;
        }
        if self.queue.len() >= 64 {
            return false;
        }
        self.queue.push_back(frame);
        true
    }

    pub fn take(&mut self) -> Option<(u64, Frame)> {
        if self.phase != Phase::Ready || self.flight.is_some() {
            return None;
        }
        let frame = self.queue.pop_front()?;
        self.next_id += 1;
        self.flight = Some((self.next_id, Instant::now()));
        Some((self.next_id, frame))
    }

    pub fn ack(&mut self, id: u64) -> bool {
        if self.flight.is_some_and(|(current, _)| current == id) {
            tracing::trace!(
                id,
                elapsed_us = self.flight.as_ref().unwrap().1.elapsed().as_micros(),
                "webview dispatch round trip"
            );
            self.flight = None;
            true
        } else {
            false
        }
    }

    pub fn needs_watch(&self) -> bool {
        self.flight.is_some() || matches!(self.phase, Phase::Loading | Phase::Recovering)
    }

    pub fn timed_out(&self) -> bool {
        self.flight
            .is_some_and(|(_, start)| start.elapsed() > Duration::from_secs(3))
            || (matches!(self.phase, Phase::Loading | Phase::Recovering)
                && self.started.elapsed() > Duration::from_secs(15))
    }

    pub fn recovery_delay(&self) -> Duration {
        Duration::from_millis(250 * (1 << self.failures.len().saturating_sub(1)))
    }

    pub fn recover(&mut self) -> bool {
        self.failures.retain(|time| time.elapsed() < Duration::from_secs(60));
        self.queue.clear();
        self.flight = None;
        if self.failures.len() >= 3 {
            self.phase = Phase::Failed;
            return false;
        }
        self.failures.push_back(Instant::now());
        self.started = Instant::now();
        self.phase = Phase::Recovering;
        true
    }
}
pub fn acknowledged_script(epoch: u64, id: u64, script: &str) -> String {
    format!(
        "(() => {{ if (window.figRuntime?.epoch !== '{epoch}') return; let failed = false; const onError = () => {{ failed = true; }}; window.addEventListener('error', onError); try {{ window.figDeliveryId = '{id}'; {script} if (failed) throw new Error('event handler failed'); window.ipc.postMessage('fig-runtime:ack:{epoch}:{id}'); }} catch (_) {{ window.ipc.postMessage('fig-runtime:nack:{epoch}:{id}'); }} finally {{ window.removeEventListener('error', onError); }} }})();"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit(buffer: &str) -> Frame {
        Frame::Edit {
            channel: 1,
            value: Box::new(EditBufferChangedNotification {
                buffer: Some(buffer.into()),
                ..Default::default()
            }),
        }
    }
    #[test]
    fn webview_backpressure_preserves_barriers_and_rejects_old_ack() {
        let mut d = Delivery::default();
        d.ready();
        d.push(edit("a"));
        let (first, _) = d.take().unwrap();
        for _ in 0..1000 {
            assert!(d.push(edit("latest")));
        }
        d.push(Frame::Script("key".into()));
        d.push(edit("after key"));
        assert_eq!(d.queue.len(), 3);
        assert!(d.take().is_none());
        assert!(!d.ack(first + 1));
        assert!(d.ack(first));
        assert!(matches!(d.take().unwrap().1, Frame::Edit { value, .. } if value.buffer.as_deref() == Some("latest")));
        d.reset();
        d.ready();
        d.push(edit("new page"));
        let (new, _) = d.take().unwrap();
        assert!(!d.ack(first));
        assert!(d.ack(new));
    }
    #[test]
    fn overload_and_recovery_are_bounded() {
        let mut d = Delivery::default();
        d.ready();
        for _ in 0..64 {
            assert!(d.push(Frame::Script("key".into())));
        }
        assert!(!d.push(Frame::Script("overflow".into())));
        for attempt in 0..3 {
            assert!(d.recover());
            assert_eq!(d.recovery_delay(), Duration::from_millis(250 * (1 << attempt)));
            d.ready();
        }
        assert!(!d.recover());
        assert_eq!(d.phase, Phase::Failed);
    }
}
