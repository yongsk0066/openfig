use dashmap::DashMap;
use fnv::FnvBuildHasher;
use serde_json::Value;
use tokio::sync::broadcast::{
    Receiver,
    Sender,
};

const CHANNEL_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub enum JsonNotification {
    NewValue { new: Value },
    ChangedValue { old: Value, new: Value },
    RemoveValue { old: Value },
}

impl JsonNotification {
    pub fn value(self) -> Option<Value> {
        match self {
            JsonNotification::NewValue { new } => Some(new),
            JsonNotification::ChangedValue { new, .. } => Some(new),
            JsonNotification::RemoveValue { .. } => None,
        }
    }

    pub fn as_bool(self) -> Option<bool> {
        self.value().and_then(|value| value.as_bool())
    }

    pub fn as_string(self) -> Option<String> {
        self.value().and_then(|value| value.as_str().map(|s| s.into()))
    }
}

/// Fan-out for `settings.json` changes, from the file watcher to the webview watchers.
#[derive(Debug, Default)]
pub struct NotificationBus {
    settings_channels: DashMap<String, Sender<JsonNotification>, FnvBuildHasher>,
}

impl NotificationBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe_settings(&self, key: String) -> Receiver<JsonNotification> {
        self.settings_channels
            .entry(key)
            .or_insert_with(|| {
                let (tx, _) = tokio::sync::broadcast::channel(CHANNEL_SIZE);
                tx
            })
            .subscribe()
    }

    pub fn send_settings(&self, key: impl AsRef<str>, value: JsonNotification) {
        if let Some(tx) = self.settings_channels.get(key.as_ref()) {
            tx.send(value).ok();
        }
    }

    pub fn send_settings_new(&self, key: impl AsRef<str>, value: &Value) {
        if let Some(tx) = self.settings_channels.get(key.as_ref()) {
            tx.send(JsonNotification::NewValue { new: value.clone() }).ok();
        }
    }

    pub fn send_settings_remove(&self, key: impl AsRef<str>, value: &Value) {
        if let Some(tx) = self.settings_channels.get(key.as_ref()) {
            tx.send(JsonNotification::RemoveValue { old: value.clone() }).ok();
        }
    }

    pub fn send_settings_changed(&self, key: impl AsRef<str>, old: &Value, new: &Value) {
        if let Some(tx) = self.settings_channels.get(key.as_ref()) {
            tx.send(JsonNotification::ChangedValue {
                old: old.clone(),
                new: new.clone(),
            })
            .ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn owners_do_not_share_notifications_and_drop_closes_their_subscriptions() {
        let first = NotificationBus::new();
        let second = NotificationBus::new();
        let mut a = first.subscribe_settings("key".into());
        let mut b = second.subscribe_settings("key".into());
        first.send_settings_new("key", &Value::Bool(true));
        assert_eq!(a.recv().await.unwrap().as_bool(), Some(true));
        assert!(b.try_recv().is_err());
        drop(first);
        assert!(matches!(
            a.recv().await,
            Err(tokio::sync::broadcast::error::RecvError::Closed)
        ));
        second.send_settings_new("key", &Value::Bool(false));
        assert_eq!(b.recv().await.unwrap().as_bool(), Some(false));
    }
}
