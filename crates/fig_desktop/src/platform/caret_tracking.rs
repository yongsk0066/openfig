//! Reuse a verified window-relative caret only across pure window translations.
use std::time::{
    Duration,
    Instant,
};

use core_graphics::geometry::CGRect;

use super::caret_worker::AnchorRect;
use super::focus_validation::FocusTarget;

#[derive(Debug, Default)]
pub struct CaretTracking {
    anchor: Option<(FocusTarget, AnchorRect, CGRect)>,
    moved_at: Option<Instant>,
}

impl CaretTracking {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.moved_at
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(150))
    }

    pub fn remember(&mut self, target: FocusTarget, rect: AnchorRect, frame: CGRect, now: Instant) {
        let contained = rect.x >= frame.origin.x
            && rect.y >= frame.origin.y
            && rect.x + rect.width <= frame.origin.x + frame.size.width
            && rect.y + rect.height <= frame.origin.y + frame.size.height;
        if !self.moving(now) && contained {
            self.anchor = Some((target, rect, frame));
        }
    }

    pub fn translate(&mut self, target: FocusTarget, frame: CGRect, now: Instant) -> Option<AnchorRect> {
        let (owner, rect, before) = self.anchor.as_mut()?;
        if *owner != target || before.size.width != frame.size.width || before.size.height != frame.size.height {
            self.clear();
            return None;
        }
        let dx = frame.origin.x - before.origin.x;
        let dy = frame.origin.y - before.origin.y;
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        rect.x += dx;
        rect.y += dy;
        *before = frame;
        self.moved_at = Some(now);
        Some(*rect)
    }
}

#[cfg(test)]
mod tests {
    use core_graphics::geometry::{
        CGPoint,
        CGSize,
    };

    use super::*;

    fn frame(x: f64, y: f64, width: f64) -> CGRect {
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(width, 600.0))
    }
    fn target() -> FocusTarget {
        FocusTarget {
            pid: 7,
            window_id: 9,
            generation: 1,
        }
    }

    #[test]
    fn follows_translation_without_accumulating_error_or_accepting_in_flight_screen_rects() {
        let now = Instant::now();
        let mut state = CaretTracking::default();
        let caret = AnchorRect {
            x: 125.0,
            y: 150.0,
            width: 0.0,
            height: 18.0,
        };
        state.remember(target(), caret, frame(100.0, 100.0, 800.0), now);
        let moved = state.translate(target(), frame(-200.0, 50.0, 800.0), now).unwrap();
        assert_eq!((moved.x, moved.y), (-175.0, 100.0));
        state.remember(target(), caret, frame(100.0, 100.0, 800.0), now);
        let back = state.translate(target(), frame(100.0, 100.0, 800.0), now).unwrap();
        assert_eq!((back.x, back.y), (caret.x, caret.y));
        assert!(!state.moving(now + Duration::from_millis(150)));
    }

    #[test]
    fn resize_focus_and_edit_invalidation_cannot_reuse_a_caret() {
        let now = Instant::now();
        for (owner, width, clear) in [
            (target(), 900.0, false),
            (
                FocusTarget {
                    generation: 2,
                    ..target()
                },
                800.0,
                false,
            ),
            (
                FocusTarget {
                    window_id: 10,
                    ..target()
                },
                800.0,
                false,
            ),
            (FocusTarget { pid: 8, ..target() }, 800.0, false),
            (target(), 800.0, true),
        ] {
            let mut state = CaretTracking::default();
            state.remember(
                target(),
                AnchorRect {
                    x: 125.0,
                    y: 150.0,
                    width: 0.0,
                    height: 18.0,
                },
                frame(100.0, 100.0, 800.0),
                now,
            );
            if clear {
                state.clear();
            }
            assert!(state.translate(owner, frame(200.0, 100.0, width), now).is_none());
            assert!(state.translate(target(), frame(300.0, 100.0, 800.0), now).is_none());
        }
    }
}
