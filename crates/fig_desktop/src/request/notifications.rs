use fig_proto::fig::{
    NotificationRequest,
    NotificationType,
};

use super::{
    RequestResult,
    RequestResultImpl,
};
use crate::WebviewNotificationsState;
use crate::webview::WindowId;

pub async fn handle_request(
    request: NotificationRequest,
    window_id: WindowId,
    message_id: i64,
    state: &WebviewNotificationsState,
) -> RequestResult {
    let Some(raw_type) = request.r#type else {
        return RequestResult::error("NotificationRequest is missing a type");
    };

    let notification_type = match NotificationType::try_from(raw_type) {
        Ok(notification_type) => notification_type,
        Err(_) => return RequestResult::error(format!("Unknown notification type {raw_type}")),
    };

    if request.subscribe.unwrap_or(true) {
        subscribe(window_id, message_id, notification_type, state)
    } else {
        unsubscribe(&window_id, notification_type, state)
    }
}

fn subscribe(
    window_id: WindowId,
    channel: i64,
    notification_type: NotificationType,
    state: &WebviewNotificationsState,
) -> RequestResult {
    if notification_type == NotificationType::All {
        return RequestResult::error("Cannot subscribe to All notification type");
    }

    let entry = state.subscriptions.entry(window_id).or_default();
    if entry.contains_key(&notification_type) {
        return RequestResult::error(format!("Already subscribed to notification type {notification_type:?}",));
    }

    entry.insert(notification_type, channel);

    RequestResult::success()
}

fn unsubscribe(
    window_id: &WindowId,
    notification_type: NotificationType,
    state: &WebviewNotificationsState,
) -> RequestResult {
    if notification_type == NotificationType::All {
        return unsubscribe_all(window_id, state);
    }

    match state.subscriptions.get(window_id) {
        Some(subscriptions) if !subscriptions.contains_key(&notification_type) => {
            return RequestResult::error(format!("Not subscribed notification type {notification_type:?}",));
        },
        None => {
            return RequestResult::error(format!("Not subscribed notification type {notification_type:?}",));
        },
        Some(subscriptions) => {
            subscriptions.remove(&notification_type);
        },
    }

    RequestResult::success()
}

fn unsubscribe_all(window_id: &WindowId, state: &WebviewNotificationsState) -> RequestResult {
    if let Some(subscriptions) = state.subscriptions.get(window_id) {
        subscriptions.clear();
    }

    RequestResult::success()
}

#[cfg(test)]
mod tests {
    use fig_proto::fig::server_originated_message::Submessage;

    use super::*;

    fn error_message(result: RequestResult) -> String {
        match *result.expect("handle_request must not return Err") {
            Submessage::Error(msg) => msg,
            other => panic!("expected an Error submessage, got {other:?}"),
        }
    }

    /// A webview that sends `{}` for a notification request must get an error back, not a
    /// panicking task that leaves the JS `fetch` pending forever.
    #[tokio::test]
    async fn missing_type_is_an_error() {
        let state = WebviewNotificationsState::default();
        let result = handle_request(
            NotificationRequest {
                subscribe: Some(true),
                r#type: None,
            },
            WindowId("test".into()),
            0,
            &state,
        )
        .await;

        assert_eq!(error_message(result), "NotificationRequest is missing a type");
        assert!(state.subscriptions.is_empty());
    }

    /// Same for an enum value this build does not know about.
    #[tokio::test]
    async fn unknown_type_is_an_error() {
        let state = WebviewNotificationsState::default();
        let result = handle_request(
            NotificationRequest {
                subscribe: Some(true),
                r#type: Some(9999),
            },
            WindowId("test".into()),
            0,
            &state,
        )
        .await;

        assert_eq!(error_message(result), "Unknown notification type 9999");
        assert!(state.subscriptions.is_empty());
    }

    #[tokio::test]
    async fn known_type_subscribes() {
        let state = WebviewNotificationsState::default();
        let result = handle_request(
            NotificationRequest {
                subscribe: Some(true),
                r#type: Some(NotificationType::NotifyOnPrompt as i32),
            },
            WindowId("test".into()),
            7,
            &state,
        )
        .await;

        assert!(matches!(*result.unwrap(), Submessage::Success(true)));
        assert_eq!(
            state.subscriptions.get(&WindowId("test".into())).unwrap().len(),
            1,
            "the known type must actually be recorded"
        );
    }
}
