//! Query cadence, separate from the cheap main-loop mouse-state observation.
use std::time::{
    Duration,
    Instant,
};

#[derive(Debug, Default)]
pub struct FrameWatch {
    queried_at: Option<Instant>,
    moved_at: Option<Instant>,
}

impl FrameWatch {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn changed(&mut self, now: Instant) {
        self.moved_at = Some(now);
    }

    pub fn due(&mut self, now: Instant, tracking: bool, visible: bool, dragging: bool) -> bool {
        if !tracking {
            self.clear();
            return false;
        }
        let active = visible
            && (dragging
                || self
                    .moved_at
                    .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(500)));
        let period = Duration::from_millis(if active { 50 } else { 250 });
        if self
            .queried_at
            .is_some_and(|at| now.saturating_duration_since(at) < period)
        {
            return false;
        }
        self.queried_at = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_queries_drop_eighty_percent_and_drag_keeps_twenty_hz() {
        let now = Instant::now();
        for (dragging, expected) in [(false, 40), (true, 200)] {
            let mut watch = FrameWatch::default();
            let count = (0..200)
                .filter(|tick| watch.due(now + Duration::from_millis(tick * 50), true, true, dragging))
                .count();
            assert_eq!(count, expected);
        }
    }

    #[test]
    fn first_drag_tick_does_not_wait_for_idle_deadline_and_settling_is_bounded() {
        let now = Instant::now();
        let mut watch = FrameWatch::default();
        assert!(watch.due(now, true, true, false));
        assert!(!watch.due(now + Duration::from_millis(50), true, true, false));
        assert!(watch.due(now + Duration::from_millis(100), true, true, true));
        watch.changed(now + Duration::from_millis(100));
        assert!(watch.due(now + Duration::from_millis(150), true, true, false));
        assert!(watch.due(now + Duration::from_millis(550), true, true, false));
        assert!(!watch.due(now + Duration::from_millis(600), true, true, false));
        assert!(watch.due(now + Duration::from_millis(800), true, true, false));
    }

    #[test]
    fn dismissal_resets_deadlines_and_hidden_geometry_stays_bounded() {
        let now = Instant::now();
        let mut watch = FrameWatch::default();
        assert!(watch.due(now, true, true, true));
        assert!(!watch.due(now, false, true, true));
        assert!(watch.due(now, true, false, true));
        watch.changed(now);
        assert!(!watch.due(now + Duration::from_millis(50), true, false, true));
        assert!(watch.due(now + Duration::from_millis(250), true, false, true));
    }
}
