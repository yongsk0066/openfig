//! Serial, latest-request focus and frame queries. Only owned identities/results cross threads.
pub use fig_desktop::native::FocusTarget;

#[derive(Debug, PartialEq, Eq)]
pub enum FocusChange {
    Stale,
    Duplicate,
    Changed,
}

pub fn classify_focus_change(
    front: Option<&macos_utils::window_server::ApplicationSpecifier>,
    incoming: &macos_utils::window_server::ApplicationSpecifier,
    window_id: Option<u32>,
    current: Option<(i32, &str, u32)>,
) -> FocusChange {
    if front != Some(incoming) {
        FocusChange::Stale
    } else if current.is_some_and(|(pid, bundle, window)| {
        pid == incoming.pid && bundle == incoming.bundle_id && Some(window) == window_id
    }) {
        FocusChange::Duplicate
    } else {
        FocusChange::Changed
    }
}

pub fn should_clear(current: FocusTarget, requested: FocusTarget, focused: Option<u32>) -> bool {
    current == requested && focused != Some(current.window_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_late_result_cannot_clear_a_new_focus_even_if_the_id_is_reused() {
        let a = FocusTarget {
            pid: 1,
            window_id: 10,
            generation: 3,
        };
        assert!(!should_clear(a, a, Some(10)));
        assert!(should_clear(a, a, None));
        assert!(should_clear(a, a, Some(11)));
        assert!(!should_clear(FocusTarget { generation: 4, ..a }, a, None));
        assert!(!should_clear(FocusTarget { pid: 2, ..a }, a, None));
    }
    #[test]
    fn queued_focus_events_are_classified_before_any_hide_or_invalidation() {
        use macos_utils::window_server::ApplicationSpecifier;
        let a = ApplicationSpecifier {
            pid: 10,
            bundle_id: "terminal.a".into(),
        };
        let b = ApplicationSpecifier {
            pid: 11,
            bundle_id: "terminal.b".into(),
        };
        let current = Some((b.pid, b.bundle_id.as_str(), 22));
        assert_eq!(
            classify_focus_change(Some(&b), &a, Some(21), current),
            FocusChange::Stale
        );
        assert_eq!(classify_focus_change(None, &b, Some(22), current), FocusChange::Stale);
        assert_eq!(
            classify_focus_change(Some(&b), &b, Some(22), current),
            FocusChange::Duplicate
        );
        assert_eq!(
            classify_focus_change(Some(&b), &b, Some(23), current),
            FocusChange::Changed
        );
        assert_eq!(classify_focus_change(Some(&b), &b, None, current), FocusChange::Changed);
        let reused = ApplicationSpecifier {
            pid: b.pid,
            bundle_id: "another.process".into(),
        };
        assert_eq!(
            classify_focus_change(Some(&reused), &b, Some(22), current),
            FocusChange::Stale
        );
    }
}
