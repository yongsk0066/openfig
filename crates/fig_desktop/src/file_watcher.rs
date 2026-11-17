use std::sync::Arc;

use fig_proto::fig::notification::Type as NotificationEnum;
use fig_proto::fig::{
    NotificationType,
    SettingsChangedNotification,
};
use fig_settings::JsonStore;
use fig_util::directories;
use notify::{
    EventKind,
    RecursiveMode,
    Watcher,
};
use serde_json::{
    Map,
    Value,
};
use tracing::{
    debug,
    error,
    warn,
};

use crate::EventLoopProxy;
use crate::notification_bus::NotificationBus;
use crate::webview::notification::WebviewNotificationsState;

fn signal_settings_change(event: &notify::Event, path: &std::path::Path, dirty: &tokio::sync::Notify) {
    if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) && event.paths.iter().any(|p| p == path)
        || event.need_rescan()
    {
        dirty.notify_one();
    }
}

pub fn setup_listeners(
    tasks: &mut tokio::task::JoinSet<()>,
    notifications_state: Arc<WebviewNotificationsState>,
    proxy: EventLoopProxy,
    bus: Arc<NotificationBus>,
) {
    let Ok(path) = directories::settings_path() else {
        error!("Settings path unavailable");
        return;
    };
    let dirty = Arc::new(tokio::sync::Notify::new());
    let callback_dirty = dirty.clone();
    let watched_path = path.clone();
    let watcher = notify::recommended_watcher(move |res| match res {
        Ok(event) => signal_settings_change(&event, &watched_path, &callback_dirty),
        Err(err) => error!(%err, "notify watcher"),
    });
    let mut watcher = match watcher {
        Ok(watcher) => watcher,
        Err(err) => {
            error!(%err, "Unable to create settings watcher");
            return;
        },
    };

    let Some(parent) = path.parent() else { return };
    if let Err(err) = watcher.watch(parent, RecursiveMode::NonRecursive) {
        error!(%err, "Failed to watch settings directory");
        return;
    }
    tasks.spawn(async move {
        let _watcher = watcher;

        let mut prev_settings = match fig_settings::OldSettings::load() {
            Ok(settings) => settings.map().clone(),
            Err(err) => {
                error!(?err, "failed to initialize settings");
                Map::new()
            },
        };

        // Reconcile the cached snapshot with writes concurrent with watcher registration.
        dirty.notify_one();
        loop {
            dirty.notified().await;
            // Refresh the shared backend before notifying observers so a later desktop
            // write preserves settings added by the CLI.
            match fig_settings::OldSettings::load_into_global()
                .and_then(|()| fig_settings::OldSettings::load())
                .map(|settings| settings.map().clone())
            {
                Ok(settings) => {
                    if settings == prev_settings {
                        continue;
                    }
                    debug!("Settings file changed");

                    if let Err(err) = notifications_state
                        .broadcast_notification_all(
                            &NotificationType::NotifyOnSettingsChange,
                            fig_proto::fig::Notification {
                                r#type: Some(NotificationEnum::SettingsChangedNotification(
                                    SettingsChangedNotification {
                                        json_blob: serde_json::to_string(&settings).ok(),
                                    },
                                )),
                            },
                            &proxy,
                        )
                        .await
                    {
                        warn!(%err, "Failed to broadcast settings change notification");
                    }

                    json_map_diff(
                        &prev_settings,
                        &settings,
                        |key, value| {
                            debug!(%key, %value, "Setting added");
                            bus.send_settings_new(key, value);
                        },
                        |key, old, new| {
                            debug!(%key, %old, %new, "Setting change");
                            bus.send_settings_changed(key, old, new);
                        },
                        |key, value| {
                            debug!(%key, %value, "Setting removed");
                            bus.send_settings_remove(key, value);
                        },
                    );

                    prev_settings = settings;
                },
                Err(err) => error!(%err, "Failed to get settings"),
            }
        }
    });
}

fn json_map_diff(
    map_a: &Map<String, Value>,
    map_b: &Map<String, Value>,
    on_new: impl Fn(&str, &Value),
    on_changed: impl Fn(&str, &Value, &Value),
    on_removed: impl Fn(&str, &Value),
) {
    for (key, value) in map_a {
        if let Some(other_value) = map_b.get(key) {
            if value != other_value {
                on_changed(key, value, other_value);
            }
        } else {
            on_removed(key, value);
        }
    }

    for (key, value) in map_b {
        if !map_a.contains_key(key) {
            on_new(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn changes_coalesce_while_the_reader_is_busy() {
        use std::path::Path;

        use notify::event::ModifyKind;
        let path = Path::new("/settings.json");
        let dirty = tokio::sync::Notify::new();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.into());
        let wait = || tokio::time::timeout(std::time::Duration::from_millis(1), dirty.notified());
        signal_settings_change(&event, Path::new("/other"), &dirty);
        assert!(wait().await.is_err());
        for _ in 0..1000 {
            signal_settings_change(&event, path, &dirty);
        }
        assert!(wait().await.is_ok());
        assert!(wait().await.is_err());
        signal_settings_change(&event, path, &dirty);
        assert!(wait().await.is_ok());
    }
}
