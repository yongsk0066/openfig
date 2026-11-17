//! A cooperative deadline for a chain of AX calls, including tree traversal.
use std::time::{
    Duration,
    Instant,
};

use accessibility::AXUIElement;
use core_foundation::base::TCFType;

pub struct QueryBudget {
    deadline: Instant,
    remaining_calls: usize,
}
impl QueryBudget {
    pub fn new(duration: Duration, max_calls: usize) -> Self {
        Self {
            deadline: Instant::now() + duration,
            remaining_calls: max_calls,
        }
    }

    pub fn exhausted(&self) -> bool {
        self.remaining_calls == 0 || Instant::now() >= self.deadline
    }

    fn claim(&mut self, now: Instant) -> Option<Duration> {
        let remaining = self.deadline.checked_duration_since(now)?;
        if self.remaining_calls == 0 || remaining.is_zero() {
            return None;
        }
        self.remaining_calls -= 1;
        Some(remaining)
    }

    pub fn run<T>(&mut self, element: &AXUIElement, operation: impl FnOnce() -> Option<T>) -> Option<T> {
        let remaining = self.claim(Instant::now())?;
        // SAFETY: element owns a live reference. This changes only this AX handle's timeout,
        // not the process-wide timeout. Positive remaining time avoids resetting to default.
        let status = unsafe {
            accessibility_sys::AXUIElementSetMessagingTimeout(
                element.as_concrete_TypeRef(),
                remaining.as_secs_f32().max(f32::MIN_POSITIVE),
            )
        };
        if status != 0 {
            return None;
        }
        let value = operation()?;
        (Instant::now() < self.deadline).then_some(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_deadline_is_shared_across_a_chain_instead_of_reset_per_call() {
        let start = Instant::now();
        let mut budget = QueryBudget {
            deadline: start + Duration::from_millis(100),
            remaining_calls: 512,
        };
        let remaining: Vec<_> = [0, 30, 75, 100, 101]
            .into_iter()
            .map(|elapsed| budget.claim(start + Duration::from_millis(elapsed)))
            .collect();
        assert_eq!(remaining, vec![
            Some(Duration::from_millis(100)),
            Some(Duration::from_millis(70)),
            Some(Duration::from_millis(25)),
            None,
            None
        ]);
    }
    #[test]
    fn very_fast_or_cyclic_trees_still_have_a_call_bound() {
        let now = Instant::now();
        let mut budget = QueryBudget {
            deadline: now + Duration::from_secs(1),
            remaining_calls: 3,
        };
        assert_eq!((0..1000).filter(|_| budget.claim(now).is_some()).count(), 3);
    }
    #[test]
    fn exhausted_budget_does_not_start_native_work() {
        assert!(QueryBudget::new(Duration::ZERO, 500).exhausted());
        assert!(QueryBudget::new(Duration::from_secs(1), 0).exhausted());
    }
}
