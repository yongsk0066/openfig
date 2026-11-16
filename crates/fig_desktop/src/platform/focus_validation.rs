//! Serial, latest-request focus validation. Only numeric identities cross the worker boundary.
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusTarget {
    pub pid: i32,
    pub window_id: u32,
    pub generation: u64,
}

pub fn start(
    query: impl Fn(i32) -> Option<u32> + Send + Sync + 'static,
    publish: impl Fn(FocusTarget, Option<u32>) + Send + 'static,
) -> tokio::sync::watch::Sender<Option<FocusTarget>> {
    let (sender, mut receiver) = tokio::sync::watch::channel(None::<FocusTarget>);
    let query = Arc::new(query);
    tokio::spawn(async move {
        while receiver.changed().await.is_ok() {
            let Some(target) = *receiver.borrow_and_update() else {
                continue;
            };
            let query = query.clone();
            // At most one blocking AX query at a time, with only the latest waiting request.
            if let Ok(result) = tokio::task::spawn_blocking(move || query(target.pid)).await {
                publish(target, result);
            }
        }
    });
    sender
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
    #[tokio::test]
    async fn blocked_validation_does_not_block_runtime_and_coalesces_waiters() {
        let (entered, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = std::sync::Mutex::new(gate);
        let (done, mut results) = tokio::sync::mpsc::unbounded_channel();
        let sender = start(
            move |pid| {
                entered.send(pid).unwrap();
                if pid == 1 {
                    gate.lock().unwrap().recv().unwrap();
                }
                Some(pid as u32)
            },
            move |target, result| {
                done.send((target.pid, result)).unwrap();
            },
        );
        let request = |pid| {
            Some(FocusTarget {
                pid,
                window_id: pid as u32,
                generation: pid as u64,
            })
        };
        sender.send_replace(request(1));
        assert_eq!(starts.recv().await, Some(1));
        sender.send_replace(request(2));
        sender.send_replace(request(3));
        // This task runs on a single-thread runtime while query(1) is blocked.
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        assert_eq!(results.recv().await, Some((1, Some(1))));
        assert_eq!(starts.recv().await, Some(3));
        assert_eq!(results.recv().await, Some((3, Some(3))));
        drop(sender);
    }
}
