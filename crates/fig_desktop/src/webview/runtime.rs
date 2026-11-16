//! Runtime lifecycle and acknowledged delivery; all methods run on the main event loop.
use fig_proto::fig::NotificationType;
use fig_shell_ipc::session::{
    ShellCommand,
    ShellSessions,
};
use tracing::warn;

use super::notification::WebviewNotificationsState;
use super::window::WindowState;
use crate::event::WindowEvent;
use crate::platform::PlatformState;
impl WindowState {
    pub(super) fn handle_runtime(
        &self,
        event: WindowEvent,
        shell_sessions: &ShellSessions,
        platform_state: &PlatformState,
        notifications_state: &WebviewNotificationsState,
    ) {
        match event {
            WindowEvent::RuntimeFatal(epoch) => {
                if epoch == self.page_epoch.current() {
                    self.request_recovery();
                }
            },
            WindowEvent::RuntimeReload(epoch) => {
                if epoch == self.page_epoch.current()
                    && self.delivery.lock().phase == super::delivery::Phase::Recovering
                    && let Err(error) = self.webview.load_url(self.url.lock().as_str())
                {
                    warn!(%error, "WebKit reload failed");
                }
            },
            WindowEvent::RuntimeReset | WindowEvent::RuntimeRecover => {
                if matches!(event, WindowEvent::RuntimeRecover) {
                    self.page_epoch.renew();
                }
                self.recovery_queued.store(false, std::sync::atomic::Ordering::Relaxed);
                if let Some(sub) = notifications_state.subscriptions.get(&self.window_id) {
                    sub.clear();
                    sub.reset_edit_buffer_context();
                }
                shell_sessions.broadcast(|| ShellCommand::InterceptFigJs {
                    intercept_keystrokes: false,
                    intercept_global_keystrokes: false,
                    actions: vec![],
                    override_actions: true,
                });
                self.window.set_visible(false);
                platform_state.set_autocomplete_visible(false);
                self.invalidate_geometry();
                let reload = if matches!(event, WindowEvent::RuntimeRecover) {
                    self.delivery.lock().recover()
                } else {
                    self.delivery.lock().reset();
                    false
                };
                self.update_watch();
                if matches!(event, WindowEvent::RuntimeReset) {
                    self.eval("window.figRuntime?.hello();");
                }
                if reload {
                    warn!("Recovering autocomplete WebKit runtime");
                    let delay = self.delivery.lock().recovery_delay();
                    let epoch = self.page_epoch.current();
                    let proxy = self.proxy.clone();
                    let window_id = self.window_id.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = proxy.send_event(crate::event::Event::WindowEvent {
                            window_id,
                            window_event: WindowEvent::RuntimeReload(epoch),
                        });
                    });
                }
            },
            WindowEvent::RuntimeHello { epoch, nonce } => {
                if epoch == self.page_epoch.current() {
                    let nonce = serde_json::to_string(&nonce).expect("string serializes");
                    self.eval(&format!("window.figRuntime?.bind({nonce}, '{epoch}');"));
                }
            },
            WindowEvent::RuntimeReady(epoch) => {
                if epoch != self.page_epoch.current() || self.delivery.lock().phase == super::delivery::Phase::Ready {
                    return;
                }
                tracing::info!(epoch, "Autocomplete runtime ready");
                self.delivery.lock().ready();
                self.update_watch();
                // A new page has no context. Send a full authoritative snapshot immediately.
                let snapshot = shell_sessions.most_recent().map(|session| {
                    let mut context = session.context.clone();
                    if let Some(context) = context.as_mut() {
                        context.context_snapshot = Some(true);
                    }
                    fig_proto::fig::EditBufferChangedNotification {
                        session_id: Some(session.id.to_string()),
                        buffer: Some(session.edit_buffer.text.clone()),
                        cursor: session
                            .edit_buffer
                            .text
                            .get(..session.edit_buffer.cursor as usize)
                            .map(|s| s.encode_utf16().count() as i32),
                        context,
                    }
                });
                if let Some(snapshot) = snapshot
                    && let Some(sub) = notifications_state.subscriptions.get(&self.window_id)
                    && let Some(channel) = sub.get(&NotificationType::NotifyOnEditbuffferChange)
                {
                    self.emit_edit_buffer(*channel, snapshot);
                }
            },
            WindowEvent::RuntimeAck { epoch, id, success } => {
                if epoch == self.page_epoch.current() && self.delivery.lock().ack(id) {
                    if success {
                        self.flush_delivery();
                    } else {
                        warn!(epoch, id, "Webview rejected delivery; resynchronizing");
                        self.request_recovery();
                    }
                }
            },
            WindowEvent::RuntimeTick => {
                if self.delivery.lock().timed_out() {
                    self.request_recovery();
                }
            },
            _ => unreachable!("only lifecycle events enter this handler"),
        }
    }

    pub(super) fn update_watch(&self) {
        self.watch_delivery
            .store(self.delivery.lock().needs_watch(), std::sync::atomic::Ordering::Relaxed);
    }

    pub(super) fn request_recovery(&self) {
        if self.recovery_queued.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let _ = self.proxy.send_event(crate::event::Event::WindowEvent {
            window_id: self.window_id.clone(),
            window_event: WindowEvent::RuntimeRecover,
        });
    }

    pub(super) fn enqueue(&self, frame: super::delivery::Frame) {
        if !self.delivery.lock().push(frame) {
            self.request_recovery();
            return;
        }
        self.flush_delivery();
    }

    pub(super) fn flush_delivery(&self) {
        let next = self.delivery.lock().take();
        self.update_watch();
        if let Some((id, frame)) = next {
            let epoch = self.page_epoch.current();
            if let super::delivery::Frame::Edit { value, .. } = &frame {
                if crate::latency::enabled() {
                    crate::latency::record(
                        "webview-dispatch",
                        value.session_id.as_deref().unwrap_or(""),
                        value.context.as_ref().and_then(|c| c.input_revision),
                        None,
                        None,
                    );
                }
                tracing::trace!(epoch, id, session = ?value.session_id, revision = ?value.context.as_ref().and_then(|c| c.input_revision), "input webview delivery");
            }
            let script = match frame {
                super::delivery::Frame::Script(script) => script,
                super::delivery::Frame::Edit { channel, value } => Self::edit_script(channel, *value),
            };
            tracing::trace!(epoch, id, bytes = script.len(), "webview delivery");
            let script = super::delivery::acknowledged_script(self.page_epoch.current(), id, &script);
            if let Err(error) = self.webview.evaluate_script(&script) {
                warn!(%error, "WebKit delivery failed");
                self.request_recovery();
            }
        }
    }
}
