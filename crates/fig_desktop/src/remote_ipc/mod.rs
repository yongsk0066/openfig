use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use base64::prelude::*;
use bytes::BytesMut;
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    EditBufferChangedNotification,
    HistoryUpdatedNotification,
    KeybindingPressedNotification,
    LocationChangedNotification,
    Notification,
    NotificationType,
    Process,
    ProcessChangedNotification,
    ServerOriginatedMessage,
    ShellPromptReturnedNotification,
};
use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
};
use fig_proto::prost::Message;
use fig_remote_ipc::figterm::FigtermState;
use parking_lot::Mutex;
use tracing::{
    debug,
    error,
};
use uuid::Uuid;

use crate::event::{
    EmitEventName,
    Event,
    WindowEvent,
};
use crate::platform::PlatformBoundEvent;
use crate::webview::WindowId;
use crate::webview::notification::WebviewNotificationsState;
use crate::{
    AUTOCOMPLETE_ID,
    EventLoopProxy,
};

#[derive(Debug, Clone)]
pub struct RemoteHook {
    pub notifications_state: Arc<WebviewNotificationsState>,
    pub proxy: EventLoopProxy,
    /// The figterm session whose environment each window was last told about.
    ///
    /// figterm only ships the environment and aliases when its shell context is dirty, so the
    /// desktop has to notice when a *window* starts looking at a different session — switching
    /// terminal tabs — and re-send them even though that hook carried none. Shared across
    /// connections on purpose: `start_remote_ipc` clones this handler per connection, and a
    /// per-connection map would never see the switch back to an already-open session.
    pub last_sent: Arc<Mutex<HashMap<WindowId, Uuid>>>,
}

/// Fold an incoming hook context into the session's, keeping the fields the hook omitted.
///
/// An empty `environment_variables` means "unchanged" rather than "no environment": figterm
/// sends `vec![]` / `alias: None` for every hook whose context is not dirty.
fn merge_shell_context(session: &mut Option<ShellContext>, mut incoming: Option<ShellContext>) {
    if let Some(incoming) = incoming.as_mut()
        && let Some(existing) = session.as_mut()
    {
        if incoming.environment_variables.is_empty() {
            incoming.environment_variables = std::mem::take(&mut existing.environment_variables);
        }
        if incoming.alias.is_none() {
            incoming.alias = existing.alias.take();
        }
    }

    *session = incoming;
}

#[async_trait::async_trait]
impl fig_remote_ipc::RemoteHookHandler for RemoteHook {
    type Error = anyhow::Error;

    async fn edit_buffer(
        &mut self,
        hook: &EditBufferHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<()> {
        // Did figterm consider its context dirty? If so every window has to be re-told, and the
        // hook already carries the full environment.
        let hook_carried_env = hook
            .context
            .as_ref()
            .is_some_and(|context| !context.environment_variables.is_empty());

        // Collect the subscribers before emitting: the loop sends events, and holding DashMap
        // shard guards across that is needless.
        let subscribers: Vec<(WindowId, i64)> = self
            .notifications_state
            .subscriptions
            .iter()
            .filter_map(|sub| {
                sub.get(&NotificationType::NotifyOnEditbuffferChange)
                    .map(|message_id| (sub.key().clone(), *message_id))
            })
            .collect();

        // A window needs the whole environment when this hook brought one, or when it is now
        // looking at a different figterm session than the one it was last told about.
        let subscribers: Vec<(WindowId, i64, bool)> = {
            let mut last_sent = self.last_sent.lock();
            subscribers
                .into_iter()
                .map(|(window_id, message_id)| {
                    let session_changed = last_sent.insert(window_id.clone(), session_id) != Some(session_id);
                    (window_id, message_id, hook_carried_env || session_changed)
                })
                .collect()
        };
        let any_full = subscribers.iter().any(|(_, _, needs_full)| *needs_full);

        // Clone before taking the session lock. `clone_from` inside the closure held the
        // `FairMutex` for the whole copy (13 KB per keystroke before F-01) while
        // `request/window.rs`'s `most_recent()` contended for it.
        let text = hook.text.clone();
        let cursor = hook.cursor;
        let terminal_cursor_coordinates = hook.terminal_cursor_coordinates;
        let context = hook.context.clone();

        let full_context = figterm_state
            .with_update(session_id, |session| {
                session.edit_buffer.text = text;
                session.edit_buffer.cursor = cursor;
                session.terminal_cursor_coordinates = terminal_cursor_coordinates;
                merge_shell_context(&mut session.context, context);

                // Only pay for the big copy when a window is actually going to receive it.
                if any_full { session.context.clone() } else { None }
            })
            .flatten();

        // What the windows that already have the environment get: the hook's own context, which
        // is lean whenever `hook_carried_env` is false. Stripped defensively.
        let mut lean_context = if hook_carried_env { None } else { hook.context.clone() };
        if let Some(context) = lean_context.as_mut() {
            context.environment_variables.clear();
            context.alias = None;
        }

        // If the session vanished between the hook arriving and the lock, the lean context is
        // all we have.
        let mut full_context = full_context.or_else(|| lean_context.clone());

        let utf16_cursor_position = hook
            .text
            .get(..hook.cursor as usize)
            .map(|s| s.encode_utf16().count() as i32);

        // One payload copy for the whole fan-out instead of a full `hook.clone()` per window;
        // the last subscriber (in practice the only one) takes it by value.
        let mut text_payload = hook.text.clone();
        let mut subscribers = subscribers.into_iter().peekable();

        while let Some((window_id, message_id, needs_full)) = subscribers.next() {
            let last = subscribers.peek().is_none();
            let context = match (needs_full, last) {
                (true, true) => full_context.take(),
                (true, false) => full_context.clone(),
                (false, true) => lean_context.take(),
                (false, false) => lean_context.clone(),
            };
            let buffer = if last {
                std::mem::take(&mut text_payload)
            } else {
                text_payload.clone()
            };

            let message = ServerOriginatedMessage {
                id: Some(message_id),
                submessage: Some(ServerOriginatedSubMessage::Notification(Notification {
                    r#type: Some(fig_proto::fig::notification::Type::EditBufferNotification(
                        EditBufferChangedNotification {
                            context,
                            buffer: Some(buffer),
                            cursor: utf16_cursor_position,
                            session_id: Some(session_id.into()),
                        },
                    )),
                })),
            };

            let mut encoded = BytesMut::new();
            message.encode(&mut encoded)?;

            debug!(%message_id, "Sending edit buffer change notification to webview");

            self.proxy.send_event(Event::WindowEvent {
                window_id,
                window_event: WindowEvent::Emit {
                    event_name: EmitEventName::Notification,
                    payload: BASE64_STANDARD.encode(encoded).into(),
                },
            })?;
        }

        let empty_edit_buffer = hook.text.trim().is_empty();

        if !empty_edit_buffer {
            self.proxy
                .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::EditBufferChanged))?;
        }

        self.proxy.send_event(Event::WindowEvent {
            window_id: AUTOCOMPLETE_ID,
            // If editbuffer is empty, hide the autocomplete window to avoid flickering
            window_event: if empty_edit_buffer {
                WindowEvent::Hide
            } else {
                WindowEvent::Show
            },
        })?;

        Ok(())
    }

    async fn prompt(&mut self, hook: &PromptHook, session_id: Uuid, figterm_state: &Arc<FigtermState>) -> Result<()> {
        let mut cwd_changed = false;
        let mut new_cwd = None;
        figterm_state.with(&session_id, |session| {
            if let (Some(old_context), Some(new_context)) = (&session.context, &hook.context) {
                cwd_changed = old_context.current_working_directory != new_context.current_working_directory;
                new_cwd.clone_from(&new_context.current_working_directory);
            }

            session.context.clone_from(&hook.context);
        });

        if cwd_changed
            && let Err(err) = self
                .notifications_state
                .broadcast_notification_all(
                    &NotificationType::NotifyOnLocationChange,
                    Notification {
                        r#type: Some(fig_proto::fig::notification::Type::LocationChangedNotification(
                            LocationChangedNotification {
                                session_id: Some(session_id.to_string()),
                                host_name: hook.context.as_ref().and_then(|ctx| ctx.hostname.clone()),
                                user_name: None,
                                directory: new_cwd,
                            },
                        )),
                    },
                    &self.proxy,
                )
                .await
        {
            error!(%err, "Failed to broadcast LocationChangedNotification");
        }

        if let Err(err) = self
            .notifications_state
            .broadcast_notification_all(
                &NotificationType::NotifyOnPrompt,
                Notification {
                    r#type: Some(fig_proto::fig::notification::Type::ShellPromptReturnedNotification(
                        ShellPromptReturnedNotification {
                            session_id: Some(session_id.to_string()),
                            shell: hook.context.as_ref().map(|ctx| Process {
                                pid: ctx.pid,
                                executable: ctx.process_name.clone(),
                                directory: ctx.current_working_directory.clone(),
                                env: vec![],
                            }),
                        },
                    )),
                },
                &self.proxy,
            )
            .await
        {
            error!(%err, "Failed to broadcast ShellPromptReturnedNotification");
        }

        Ok(())
    }

    async fn pre_exec(
        &mut self,
        hook: &PreExecHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<()> {
        figterm_state.with_update(session_id, |session| {
            session.context.clone_from(&hook.context);
        });

        self.proxy.send_event(Event::WindowEvent {
            window_id: AUTOCOMPLETE_ID.clone(),
            window_event: WindowEvent::Hide,
        })?;

        self.notifications_state
            .broadcast_notification_all(
                &NotificationType::NotifyOnProcessChanged,
                Notification {
                    r#type: Some(fig_proto::fig::notification::Type::ProcessChangeNotification(
                        ProcessChangedNotification {
                        session_id: Some(session_id.to_string()),
                        new_process: // TODO: determine active application based on tty
                        hook.context.as_ref().map(|ctx| Process {
                            pid: ctx.pid,
                            executable: ctx.process_name.clone(),
                            directory: ctx.current_working_directory.clone(),
                            env: vec![],
                        }),
                    },
                    )),
                },
                &self.proxy,
            )
            .await?;

        Ok(())
    }

    async fn post_exec(
        &mut self,
        hook: &PostExecHook,
        session_id: Uuid,
        figterm_state: &Arc<FigtermState>,
    ) -> Result<()> {
        figterm_state.with_update(session_id, |session| {
            session.context.clone_from(&hook.context);
        });

        self.notifications_state
            .broadcast_notification_all(
                &NotificationType::NotifyOnHistoryUpdated,
                Notification {
                    r#type: Some(fig_proto::fig::notification::Type::HistoryUpdatedNotification(
                        HistoryUpdatedNotification {
                            command: hook.command.clone(),
                            process_name: hook.context.as_ref().and_then(|ctx| ctx.process_name.clone()),
                            current_working_directory: hook
                                .context
                                .as_ref()
                                .and_then(|ctx| ctx.current_working_directory.clone()),
                            session_id: Some(session_id.to_string()),
                            hostname: hook.context.as_ref().and_then(|ctx| ctx.hostname.clone()),
                            exit_code: hook.exit_code,
                        },
                    )),
                },
                &self.proxy,
            )
            .await?;

        Ok(())
    }

    async fn intercepted_key(
        &mut self,
        InterceptedKeyHook { action, context, .. }: InterceptedKeyHook,
        _session_id: Uuid,
    ) -> Result<()> {
        debug!(%action, "Intercepted Key Action");

        self.notifications_state
            .broadcast_notification_all(
                &NotificationType::NotifyOnKeybindingPressed,
                Notification {
                    r#type: Some(fig_proto::fig::notification::Type::KeybindingPressedNotification(
                        KeybindingPressedNotification {
                            keypress: None,
                            action: Some(action),
                            context,
                        },
                    )),
                },
                &self.proxy,
            )
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use fig_proto::fig::EnvironmentVariable;

    use super::*;

    fn env(name: &str) -> EnvironmentVariable {
        EnvironmentVariable {
            key: name.into(),
            value: Some("value".into()),
        }
    }

    fn context(env_names: &[&str], alias: Option<&str>) -> Option<ShellContext> {
        Some(ShellContext {
            environment_variables: env_names.iter().map(|n| env(n)).collect(),
            alias: alias.map(Into::into),
            current_working_directory: Some("/tmp".into()),
            ..Default::default()
        })
    }

    /// A hook whose context is not dirty arrives with `environment_variables: vec![]` and
    /// `alias: None`. That means "unchanged" — the session must keep what it had.
    #[test]
    fn a_lean_hook_keeps_the_session_environment() {
        let mut session = context(&["PATH", "HOME"], Some("gs=git status"));
        merge_shell_context(&mut session, context(&[], None));

        let session = session.unwrap();
        assert_eq!(session.environment_variables.len(), 2);
        assert_eq!(session.alias.as_deref(), Some("gs=git status"));
    }

    /// A dirty hook replaces both fields outright, including shrinking them.
    #[test]
    fn a_dirty_hook_replaces_the_session_environment() {
        let mut session = context(&["PATH", "HOME"], Some("gs=git status"));
        merge_shell_context(&mut session, context(&["PATH"], Some("ll=ls -l")));

        let session = session.unwrap();
        assert_eq!(session.environment_variables.len(), 1);
        assert_eq!(session.alias.as_deref(), Some("ll=ls -l"));
    }

    /// Non-environment fields always come from the hook, dirty or not.
    #[test]
    fn the_rest_of_the_context_always_comes_from_the_hook() {
        let mut session = context(&["PATH"], Some("gs=git status"));
        let mut incoming = context(&[], None).unwrap();
        incoming.current_working_directory = Some("/elsewhere".into());
        merge_shell_context(&mut session, Some(incoming));

        let session = session.unwrap();
        assert_eq!(session.current_working_directory.as_deref(), Some("/elsewhere"));
        assert_eq!(session.environment_variables.len(), 1, "env still preserved");
    }

    /// Nothing to merge into on the very first hook of a session.
    #[test]
    fn a_first_hook_is_taken_as_is() {
        let mut session = None;
        merge_shell_context(&mut session, context(&["PATH"], Some("gs=git status")));
        assert_eq!(session.unwrap().environment_variables.len(), 1);
    }

    /// A hook with no context at all clears the session's, as it always has.
    #[test]
    fn a_contextless_hook_clears_the_session_context() {
        let mut session = context(&["PATH"], None);
        merge_shell_context(&mut session, None);
        assert!(session.is_none());
    }

    /// The per-window bookkeeping behind the "session changed" half of the rule: a window is
    /// told the full environment the first time it sees a session and again whenever the
    /// session it is looking at changes, but not for repeat hooks of the same session.
    #[test]
    fn a_window_is_re_sent_the_environment_only_when_its_session_changes() {
        let window = WindowId("autocomplete".into());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut last_sent: HashMap<WindowId, Uuid> = HashMap::new();

        let mut changed = |session| last_sent.insert(window.clone(), session) != Some(session);

        assert!(changed(a), "first hook for this window");
        assert!(!changed(a), "same session again");
        assert!(changed(b), "switched to the other tab");
        assert!(!changed(b));
        assert!(changed(a), "switched back");
    }
}
