//! A page owns its API futures and queued UI effects until navigation invalidates it.
use std::future::{
    Future,
    poll_fn,
};

use parking_lot::Mutex;
use tokio::sync::watch;

pub struct PageEpoch {
    current: Mutex<u64>,
    changed: watch::Sender<u64>,
}
impl Default for PageEpoch {
    fn default() -> Self {
        Self {
            current: Mutex::new(1),
            changed: watch::channel(1).0,
        }
    }
}
impl PageEpoch {
    pub fn current(&self) -> u64 {
        *self.current.lock()
    }

    pub fn renew(&self) {
        let mut current = self.current.lock();
        *current += 1;
        self.changed.send_replace(*current);
    }

    pub async fn run<T>(&self, epoch: u64, future: impl Future<Output = T>) -> Result<T, ()> {
        let mut changed = self.changed.subscribe();
        tokio::pin!(future);
        let guarded = poll_fn(|cx| {
            // No await while locked: reset cannot interleave one synchronous effect/poll.
            let current = self.current.lock();
            if *current != epoch {
                return std::task::Poll::Ready(Err(()));
            }
            future.as_mut().poll(cx).map(Ok)
        });
        tokio::select! {
            biased;
            _ = changed.changed() => Err(()),
            result = guarded => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn old_pages_never_start_and_pending_work_is_dropped() {
        let page = PageEpoch::default();
        let old = page.current();
        page.renew();
        assert!(page.run(old, async { panic!("old request executed") }).await.is_err());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let pending = page.run(page.current(), async move {
            let _tx = tx;
            std::future::pending::<()>().await;
        });
        tokio::pin!(pending);
        poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        page.renew();
        assert!(pending.await.is_err());
        assert!(rx.await.is_err());
    }
}
