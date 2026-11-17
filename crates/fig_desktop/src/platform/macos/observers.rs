use std::sync::atomic::Ordering;

use fig_util::consts::APP_BUNDLE_ID;
use macos_utils::accessibility::accessibility_is_enabled;
use macos_utils::window_server::UIElement;
use macos_utils::{
    NotificationCenter,
    WindowServer,
    WindowServerEvent,
};
use objc2_foundation::{
    NSOperationQueue,
    ns_string,
};
use tracing::warn;

use super::{
    ACCESSIBILITY_ENABLED,
    AUTOCOMPLETE_ID,
    FigIdMap,
    PlatformBoundEvent,
    PlatformState,
};
use crate::event::{
    Event,
    WindowEvent,
};

impl PlatformState {
    pub(super) fn start_workspace_observers(&self) -> anyhow::Result<()> {
        if accessibility_is_enabled() {
            UIElement::set_global_messaging_timeout(0.25);
        }

        let (tx, rx) = flume::unbounded::<WindowServerEvent>();

        self.window_server.borrow_mut().replace(WindowServer::new(
            self.main_thread,
            tx,
            APP_BUNDLE_ID,
            fig_util::terminal::xterm_bundle_ids(),
        ));

        let accessibility_proxy = self.proxy.clone();
        let distributed = NotificationCenter::distributed_center();
        let ax_notification_name = ns_string!("com.apple.accessibility.api");
        let queue = NSOperationQueue::new();
        let subscription = distributed.subscribe(self.main_thread, ax_notification_name, Some(&queue), move |_| {
            accessibility_proxy
                .clone()
                .send_event(Event::PlatformBoundEvent(
                    PlatformBoundEvent::AccessibilityUpdateRequested,
                ))
                .ok();
        });

        self.accessibility_subscription.replace(Some(subscription));

        let observer_proxy = self.proxy.clone();
        let activation_queries = self.activation_queries.clone();
        self.observer_tasks.borrow_mut().spawn(async move {
            while let std::result::Result::Ok(result) = rx.recv_async().await {
                let mut events: Vec<Event> = vec![];

                match result {
                    WindowServerEvent::FocusRequested { app } => {
                        activation_queries.send_replace(Some(app));
                    },
                    WindowServerEvent::Workspace(event) => {
                        events.push(Event::PlatformBoundEvent(PlatformBoundEvent::WorkspaceNotification(
                            event,
                        )));
                    },
                    WindowServerEvent::WindowDestroyed { app } => {
                        events.push(Event::PlatformBoundEvent(PlatformBoundEvent::WindowDestroyed { app }));
                    },
                    WindowServerEvent::ActiveSpaceChanged => {
                        events.push(Event::WindowEvent {
                            window_id: AUTOCOMPLETE_ID,
                            window_event: WindowEvent::Hide,
                        });
                    },
                    WindowServerEvent::RequestCaretPositionUpdate => {
                        events.push(Event::PlatformBoundEvent(
                            PlatformBoundEvent::CaretPositionUpdateRequested,
                        ));
                    },
                };

                for event in events {
                    if let Err(e) = observer_proxy.send_event(event) {
                        warn!("Error sending event: {e:?}");
                    }
                }
            }
        });

        Ok(())
    }

    pub(super) fn start_window_tracking(&self, window_map: &FigIdMap) -> anyhow::Result<()> {
        if let Some(window) = window_map.get(&AUTOCOMPLETE_ID) {
            crate::platform::webview_host::install_focus_behavior(&window.webview)?;
        }

        // Observe drag state at 20 Hz; idle native frame queries run at 4 Hz.
        // Only one tick may wait in the main queue.
        let proxy = self.proxy.clone();
        let visible = self.autocomplete_visible.clone();
        let tracking = self.autocomplete_tracking.clone();
        let queued = self.frame_watch_queued.clone();
        let wake = self.frame_watch_wake.clone();
        self.observer_tasks.borrow_mut().spawn(async move {
            loop {
                if !tracking.load(Ordering::Acquire) {
                    wake.notified().await;
                    continue;
                }
                let period = if visible.load(Ordering::Acquire) { 50 } else { 250 };
                tokio::select! {
                    () = wake.notified() => continue,
                    () = tokio::time::sleep(std::time::Duration::from_millis(period)) => {},
                }
                if tracking.load(Ordering::Acquire)
                    && !queued.swap(true, Ordering::AcqRel)
                    && proxy
                        .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::FrameWatchTick))
                        .is_err()
                {
                    break;
                }
            }
        });

        // TCC may become ready after login without a settings-change notification.
        let proxy = self.proxy.clone();
        self.observer_tasks.borrow_mut().spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let enabled = accessibility_is_enabled();
                if enabled != ACCESSIBILITY_ENABLED.load(Ordering::SeqCst)
                    && proxy
                        .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::AccessibilityUpdated {
                            enabled,
                        }))
                        .is_err()
                {
                    break;
                }
            }
        });
        Ok(())
    }
}
