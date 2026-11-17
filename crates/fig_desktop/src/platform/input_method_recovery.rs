//! IMK recovery owns its selection lease and bounded retry policy; all TIS calls stay on main.
use std::time::{
    Duration,
    Instant,
};

use macos_utils::input_source::PaletteReselection;
use objc2::MainThreadMarker;

use super::PlatformBoundEvent;
use crate::EventLoopProxy;
use crate::event::{
    Event,
    EventSender,
};

#[derive(Debug)]
pub(super) struct Coordinator {
    policy: Recovery,
    selection: Option<PaletteReselection>,
    main_thread: MainThreadMarker,
    proxy: EventLoopProxy,
}
impl Coordinator {
    pub fn new(main_thread: MainThreadMarker, proxy: EventLoopProxy) -> Self {
        Self {
            policy: Recovery::default(),
            selection: None,
            main_thread,
            proxy,
        }
    }

    pub fn connected(&mut self) {
        self.policy.connected();
    }

    pub fn reconnecting(&self) -> bool {
        self.policy.reconnecting()
    }

    pub fn retiring(&mut self, now: Instant) {
        self.policy.retiring_until = Some(now + Duration::from_secs(5));
    }

    pub fn observe(&mut self, attached: Option<bool>, now: Instant) {
        if !self.policy.observe(attached, now) {
            return;
        }
        let source = format!("{}.inputmethod", fig_util::consts::APP_BUNDLE_ID);
        match PaletteReselection::begin(self.main_thread, &source) {
            Ok(Some(lease)) => {
                self.selection = Some(lease);
                let proxy = self.proxy.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    proxy.send_event_or_log(Event::PlatformBoundEvent(PlatformBoundEvent::FinishInputMethodRecovery));
                });
                tracing::info!("Reconnecting the IMK palette without restarting the terminal");
            },
            result => {
                if let Err(status) = result {
                    tracing::warn!(status, "Input method palette detach failed");
                }
                self.policy.finished(now);
            },
        }
    }

    pub fn finish(&mut self, now: Instant) {
        if let Some(lease) = self.selection.take() {
            if let Err(status) = lease.finish() {
                tracing::warn!(status, "Input method palette reselection failed");
            }
            self.policy.finished(now);
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Recovery {
    missing_since: Option<Instant>,
    next_attempt: Option<Instant>,
    failures: u8,
    pending: bool,
    reconnecting: bool,
    retiring_until: Option<Instant>,
}
impl Recovery {
    pub fn connected(&mut self) {
        self.reconnecting = true;
        self.retiring_until = None;
        self.missing_since = None;
    }

    pub fn reconnecting(&self) -> bool {
        self.reconnecting || self.pending || self.retiring_until.is_some()
    }

    /// `None` means the requested terminal is not foreground, not a broken client.
    pub fn observe(&mut self, attached: Option<bool>, now: Instant) -> bool {
        if self.retiring_until.is_some_and(|until| now < until) {
            return false;
        }
        self.retiring_until = None;
        if attached != Some(false) {
            self.missing_since = None;
            if attached == Some(true) {
                self.reconnecting = false;
                self.failures = 0;
                self.next_attempt = None;
            }
            return false;
        }
        let since = *self.missing_since.get_or_insert(now);
        if self.pending
            || now.duration_since(since) < Duration::from_millis(750)
            || self.next_attempt.is_some_and(|next| now < next)
        {
            return false;
        }
        self.pending = true;
        self.reconnecting = true;
        true
    }

    pub fn finished(&mut self, now: Instant) {
        self.pending = false;
        self.failures = self.failures.saturating_add(1);
        let seconds = (5_u64 << self.failures.min(4).saturating_sub(1)).min(30);
        self.next_attempt = Some(now + Duration::from_secs(seconds));
        self.missing_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retirement_allows_client_cleanup_and_a_new_connection_releases_the_hold() {
        let mut state = Recovery::default();
        let now = Instant::now();
        state.retiring_until = Some(now + Duration::from_secs(5));
        assert!(state.reconnecting());
        assert!(!state.observe(Some(false), now));
        assert!(!state.observe(Some(false), now + Duration::from_secs(4)));
        state.connected();
        assert!(!state.observe(Some(false), now + Duration::from_secs(4)));
        assert!(state.observe(Some(false), now + Duration::from_secs(5)));
    }
    #[test]
    fn an_unanswered_retirement_cannot_suspend_recovery_forever() {
        let mut state = Recovery::default();
        let now = Instant::now();
        state.retiring_until = Some(now + Duration::from_secs(5));
        assert!(!state.observe(Some(false), now + Duration::from_secs(5)));
        assert!(state.observe(Some(false), now + Duration::from_secs(6)));
    }
    #[test]
    fn retry_backoff_caps_at_thirty_seconds_and_health_resets_it() {
        let mut state = Recovery::default();
        let mut now = Instant::now();
        for seconds in [5, 10, 20, 30, 30] {
            state.finished(now);
            assert_eq!(state.next_attempt, Some(now + Duration::from_secs(seconds)));
            now += Duration::from_secs(seconds);
        }
        state.observe(Some(true), now);
        assert_eq!(state.next_attempt, None);
        assert_eq!(state.failures, 0);
    }
    #[test]
    fn missing_client_needs_a_stable_foreground_and_retries_are_bounded() {
        let mut state = Recovery::default();
        let start = Instant::now();
        assert!(!state.observe(Some(false), start));
        assert!(!state.observe(None, start + Duration::from_secs(1)));
        assert!(!state.observe(Some(false), start + Duration::from_secs(2)));
        assert!(state.observe(Some(false), start + Duration::from_secs(3)));
        assert!(!state.observe(Some(false), start + Duration::from_secs(4)));
        state.finished(start + Duration::from_secs(5));
        assert!(!state.observe(Some(false), start + Duration::from_secs(6)));
        assert!(!state.observe(Some(false), start + Duration::from_secs(9)));
        assert!(state.observe(Some(false), start + Duration::from_secs(10)));
    }
    #[test]
    fn attachment_does_not_cancel_an_outstanding_selection_lease() {
        let mut state = Recovery::default();
        let now = Instant::now();
        state.connected();
        assert!(state.reconnecting());
        assert!(!state.observe(Some(false), now));
        assert!(state.observe(Some(false), now + Duration::from_secs(1)));
        assert!(!state.observe(Some(true), now + Duration::from_secs(2)));
        assert!(state.reconnecting());
        state.finished(now + Duration::from_secs(3));
        assert!(!state.reconnecting());
        state.connected();
        assert!(state.reconnecting());
        assert!(!state.observe(Some(true), now + Duration::from_secs(4)));
        assert!(!state.reconnecting());
    }
}
