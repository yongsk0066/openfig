use std::sync::Arc;

use anyhow::Result;
use fig_proto::fig::{
    EditBufferChangedNotification,
    HistoryUpdatedNotification,
    KeybindingPressedNotification,
    LocationChangedNotification,
    Notification,
    NotificationType,
    Process,
    ProcessChangedNotification,
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
use fig_shell_ipc::session::ShellSessions;
use tracing::{
    debug,
    error,
};
use uuid::Uuid;

use crate::event::{
    Event,
    WindowEvent,
};
use crate::webview::WindowId;
use crate::webview::notification::WebviewNotificationsState;
use crate::{
    AUTOCOMPLETE_ID,
    EventLoopProxy,
};

#[derive(Debug, Clone)]
pub struct ShellHook {
    pub notifications_state: Arc<WebviewNotificationsState>,
    pub proxy: EventLoopProxy,
}

/// Fold an incoming hook context into the session's, keeping the fields the hook omitted.
///
/// A full snapshot replaces even empty fields; a delta preserves omitted fields.
/// Legacy senders without a snapshot flag retain the previous merge semantics.
fn merge_shell_context(session: &mut Option<ShellContext>, mut incoming: Option<ShellContext>) {
    if let Some(incoming) = incoming.as_mut()
        && let Some(existing) = session.as_mut()
    {
        if incoming.context_snapshot != Some(true) && incoming.environment_variables.is_empty() {
            incoming.environment_variables = std::mem::take(&mut existing.environment_variables);
        }
        if incoming.context_snapshot != Some(true) && incoming.alias.is_none() {
            incoming.alias = existing.alias.take();
        }
    }

    *session = incoming;
}

#[async_trait::async_trait]
impl fig_shell_ipc::ShellHookHandler for ShellHook {
    type Error = anyhow::Error;

    async fn edit_buffer(
        &mut self,
        hook: &EditBufferHook,
        session_id: Uuid,
        shell_sessions: &Arc<ShellSessions>,
    ) -> Result<()> {
        let received = std::time::Instant::now();
        if crate::latency::enabled() {
            crate::latency::record(
                "hook-received",
                &session_id.to_string(),
                hook.context.as_ref().and_then(|context| context.input_revision),
                None,
                None,
            );
        }
        // Did the session's context change? If so every window has to be re-told, and the hook
        // already carries the full environment.
        let hook_carried_env = hook
            .context
            .as_ref()
            .is_some_and(|context| context.context_snapshot == Some(true) || !context.environment_variables.is_empty());

        // Collect the subscribers before emitting: the loop sends events, and holding DashMap
        // shard guards across that is needless.
        let subscribers: Vec<(WindowId, i64, bool, u64)> = self
            .notifications_state
            .subscriptions
            .iter()
            .filter_map(|sub| {
                sub.edit_buffer_subscription(session_id, hook_carried_env)
                    .map(|(message_id, full, generation)| (sub.key().clone(), message_id, full, generation))
            })
            .collect();
        let any_full = subscribers.iter().any(|(_, _, needs_full, _)| *needs_full);

        // Allocate before locking so geometry reads are not blocked by buffer/context copies.
        let text = hook.text.clone();
        let cursor = hook.cursor;
        let terminal_cursor_coordinates = hook.terminal_cursor_coordinates;
        let context = hook.context.clone();

        let full_context = shell_sessions
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
            context.context_snapshot = Some(false);
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

        let mut deliveries = Vec::new();
        while let Some((window_id, message_id, needs_full, generation)) = subscribers.next() {
            let last = subscribers.peek().is_none();
            let context = match (needs_full, last) {
                (true, true) => full_context.take(),
                (true, false) => full_context.clone(),
                (false, true) => lean_context.take(),
                (false, false) => lean_context.clone(),
            };
            let context = context.map(|mut context| {
                context.context_snapshot = Some(needs_full);
                context
            });
            let buffer = if last {
                std::mem::take(&mut text_payload)
            } else {
                text_payload.clone()
            };

            deliveries.push(crate::event_queue::EditDelivery {
                window: window_id,
                channel: message_id,
                generation,
                notification: EditBufferChangedNotification {
                    context,
                    buffer: Some(buffer),
                    cursor: utf16_cursor_position,
                    session_id: Some(session_id.to_string()),
                },
            });
        }
        // One ordered event carries notification, caret tracking and visibility. Only an
        // adjacent edit for the same session/subscriptions may replace it; keys/commands
        // and every other proxy event close the merge window.
        self.proxy.send_edit_buffer(crate::event_queue::EditBufferBatch {
            received,
            session: session_id,
            empty: hook.text.trim().is_empty(),
            deliveries,
        })?;

        Ok(())
    }

    async fn prompt(&mut self, hook: &PromptHook, session_id: Uuid, shell_sessions: &Arc<ShellSessions>) -> Result<()> {
        self.proxy.with_edit_barrier(|| {});
        let mut cwd_changed = false;
        let mut new_cwd = None;
        shell_sessions.with(&session_id, |session| {
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
        shell_sessions: &Arc<ShellSessions>,
    ) -> Result<()> {
        self.proxy.with_edit_barrier(|| {});
        shell_sessions.with_update(session_id, |session| {
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
                            // This notification describes the shell hook process. UI foreground
                            // application identity is owned separately by PlatformState.
                            new_process: hook.context.as_ref().map(|ctx| Process {
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
        shell_sessions: &Arc<ShellSessions>,
    ) -> Result<()> {
        self.proxy.with_edit_barrier(|| {});
        shell_sessions.with_update(session_id, |session| {
            session.context.clone_from(&hook.context);
        });

        Ok(())
    }

    async fn command_finished(&mut self, command: fig_settings::history::CommandInfo) {
        let notifications = self.notifications_state.clone();
        let proxy = self.proxy.clone();
        // Publish only after the transaction commits: subscribers can now query new rows.
        // Neither disk I/O nor notification delivery blocks the next shell prompt.
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                fig_settings::history::History::new().insert_command_history(&command)?;
                Ok::<_, anyhow::Error>(command)
            })
            .await;
            match result {
                Ok(Ok(command)) => {
                    let notification = HistoryUpdatedNotification {
                        command: command.command,
                        process_name: command.shell,
                        current_working_directory: command.cwd,
                        session_id: command.session_id,
                        hostname: command.hostname,
                        exit_code: command.exit_code,
                    };
                    if let Err(err) = notifications
                        .broadcast_notification_all(
                            &NotificationType::NotifyOnHistoryUpdated,
                            Notification {
                                r#type: Some(fig_proto::fig::notification::Type::HistoryUpdatedNotification(
                                    notification,
                                )),
                            },
                            &proxy,
                        )
                        .await
                    {
                        error!(%err, "Failed to publish committed history");
                    }
                },
                result => error!(?result, "Failed to insert command into history"),
            }
        });
    }

    async fn intercepted_key(
        &mut self,
        InterceptedKeyHook { action, context, .. }: InterceptedKeyHook,
        _session_id: Uuid,
    ) -> Result<()> {
        self.proxy.with_edit_barrier(|| {});
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
    #[test]
    fn an_empty_snapshot_clears_environment_and_aliases() {
        let mut session = context(&["PATH"], Some("gs=git status"));
        let mut empty = context(&[], Some("")).unwrap();
        empty.context_snapshot = Some(true);
        merge_shell_context(&mut session, Some(empty));
        let session = session.unwrap();
        assert!(session.environment_variables.is_empty());
        assert_eq!(session.alias.as_deref(), Some(""));
    }
}
