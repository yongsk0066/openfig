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
    trace,
    warn,
};

use crate::EventLoopProxy;
use crate::notification_bus::NOTIFICATION_BUS;
use crate::webview::notification::WebviewNotificationsState;

pub async fn setup_listeners(notifications_state: Arc<WebviewNotificationsState>, proxy: EventLoopProxy) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let watcher = notify::recommended_watcher(move |res| match res {
        Ok(event) => {
            if let Err(err) = tx.send(event) {
                error!(%err, "failed to send notify event");
            }
        },
        Err(err) => error!(%err, "notify watcher"),
    });
    let mut watcher = match watcher {
        Ok(watcher) => watcher,
        Err(err) => {
            error!(%err, "Unable to create settings watcher");
            return;
        },
    };

    let settings_path = match directories::settings_path() {
        Ok(settings_path) => match settings_path.parent() {
            Some(settings_dir) => match watcher.watch(settings_dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    trace!("watching settings file at {settings_dir:?}");
                    Some(settings_path)
                },
                Err(err) => {
                    error!(%err, "failed to watch settings dir");
                    None
                },
            },
            None => {
                error!("failed to get settings file dir");
                None
            },
        },
        Err(err) => {
            error!(%err, "failed to get settings file path");
            None
        },
    };

    tokio::spawn(async move {
        let _watcher = watcher;

        let mut prev_settings = match fig_settings::OldSettings::load_from_file() {
            Ok(map) => map,
            Err(err) => {
                error!(?err, "failed to initialize settings");
                Map::new()
            },
        };

        while let Some(event) = rx.recv().await {
            trace!(?event, "Settings event");

            if let Some(settings_path) = &settings_path
                && event.paths.contains(settings_path)
                && let EventKind::Create(_) | EventKind::Modify(_) = event.kind
            {
                // Refresh the process-wide `SETTINGS_DATA` rather than only reading a local
                // copy: `settings::get_*`/`set_value` go through the global backend
                // (fig_settings::JsonStore::load returns the cached map once it exists), so a
                // watcher that only reads locally leaves the desktop blind to every CLI write —
                // and the next desktop-side write then saves a stale map, dropping the keys the
                // CLI had added.
                match fig_settings::OldSettings::load_into_global()
                    .and_then(|()| fig_settings::OldSettings::load())
                    .map(|settings| settings.map().clone())
                {
                    Ok(settings) => {
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
                                NOTIFICATION_BUS.send_settings_new(key, value);
                            },
                            |key, old, new| {
                                debug!(%key, %old, %new, "Setting change");
                                NOTIFICATION_BUS.send_settings_changed(key, old, new);
                            },
                            |key, value| {
                                debug!(%key, %value, "Setting removed");
                                NOTIFICATION_BUS.send_settings_remove(key, value);
                            },
                        );

                        prev_settings = settings;
                    },
                    Err(err) => error!(%err, "Failed to get settings"),
                }
            }
        }
    });
}

// Diffs the old and new settings and calls the appropriate callbacks
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
