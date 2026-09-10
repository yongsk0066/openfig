use anyhow::Result;
use fig_proto::local::{
    ClearAutocompleteCacheHook,
    EventHook,
};

use crate::event::{
    EventSender,
    WindowEvent,
};
use crate::webview::WindowId;
use crate::{
    AUTOCOMPLETE_ID,
    Event,
    EventLoopProxy,
};

pub async fn focus_change(proxy: &EventLoopProxy) -> Result<()> {
    proxy.send_event_or_log(Event::WindowEvent {
        window_id: AUTOCOMPLETE_ID.clone(),
        window_event: WindowEvent::Hide,
    });

    Ok(())
}

pub async fn event(hook: EventHook, proxy: &EventLoopProxy) -> Result<()> {
    let window_event = WindowEvent::Event {
        event_name: hook.event_name.into(),
        payload: hook.payload.map(|s| s.into()),
    };

    if hook.apps.is_empty() {
        proxy.send_event_or_log(Event::WindowEventAll { window_event });
    } else {
        for app in hook.apps {
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: WindowId(app.into()),
                window_event: window_event.clone(),
            });
        }
    }

    Ok(())
}

pub async fn clear_autocomplete_cache(hook: ClearAutocompleteCacheHook, proxy: &EventLoopProxy) -> Result<()> {
    proxy.send_event(Event::WindowEvent {
        window_id: AUTOCOMPLETE_ID,
        window_event: WindowEvent::Event {
            event_name: "clear-cache".into(),
            payload: Some(serde_json::to_string(&hook.clis)?.into()),
        },
    })?;

    Ok(())
}
