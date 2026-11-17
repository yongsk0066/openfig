use std::io;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tokio::sync::{
    OwnedSemaphorePermit,
    Semaphore,
    mpsc,
    oneshot,
};

use super::{
    ICON_TIMEOUT,
    Icon,
    Process,
    Reply,
    Request,
};

struct Job {
    icon: Icon,
    response: oneshot::Sender<Option<Vec<u8>>>,
    _permit: OwnedSemaphorePermit,
}

/// Application-owned icon lane: eight admitted requests, one child, no desktop event dispatch.
pub struct Icons {
    sender: mpsc::Sender<Job>,
    slots: Arc<Semaphore>,
}
impl Icons {
    pub fn new() -> io::Result<Self> {
        Ok(Self::with_process(Process::installed()?))
    }

    fn with_process(mut process: Process) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Job>(8);
        tokio::spawn(async move {
            while let Some(job) = receiver.recv().await {
                if job.response.is_closed() {
                    continue;
                }
                let response = process.call(Request::Icon(job.icon), ICON_TIMEOUT).await;
                let png = match response {
                    Ok(Reply::Icon(Some(bytes))) => STANDARD.decode(bytes).ok(),
                    _ => None,
                };
                let _ = job.response.send(png);
            }
            process.shutdown().await;
        });
        Self {
            sender,
            slots: Arc::new(Semaphore::new(8)),
        }
    }

    pub async fn render(&self, icon: Icon) -> Option<Vec<u8>> {
        let permit = self.slots.clone().try_acquire_owned().ok()?;
        let (response, receiver) = oneshot::channel();
        self.sender
            .try_send(Job {
                icon,
                response,
                _permit: permit,
            })
            .ok()?;
        tokio::time::timeout(Duration::from_secs(2), receiver)
            .await
            .ok()?
            .ok()?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stalled_icon_lane_is_bounded_and_returns_its_permits_after_caller_timeouts() {
        let (_dir, process) = super::super::client::tests::fixture("hang");
        let icons = Arc::new(Icons::with_process(process));
        let mut jobs = Vec::new();
        for _ in 0..8 {
            let icons = icons.clone();
            jobs.push(tokio::spawn(
                async move { icons.render(Icon::Named("txt".into())).await },
            ));
        }
        tokio::task::yield_now().await;
        let start = std::time::Instant::now();
        assert!(icons.render(Icon::Named("overflow".into())).await.is_none());
        assert!(start.elapsed() < Duration::from_millis(100));
        for job in jobs {
            assert!(job.await.unwrap().is_none());
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while icons.slots.available_permits() != 8 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
