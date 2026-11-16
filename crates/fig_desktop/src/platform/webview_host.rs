//! The macOS/Wry lifecycle boundary; callers exchange typed window events.
use std::sync::Arc;

use wry::{
    PageLoadEvent,
    WebViewBuilder,
    WebViewBuilderExtDarwin,
};

use crate::application::ApplicationServices;
use crate::event::{
    Event,
    WindowEvent,
};
use crate::webview::AUTOCOMPLETE_ID;

pub fn configure<'a>(builder: WebViewBuilder<'a>, state: Arc<ApplicationServices>) -> WebViewBuilder<'a> {
    let ipc = state.clone();
    let navigation = state.clone();
    builder
        .with_initialization_script(if crate::latency::enabled() {
            "window.figLatencyTrace = true;"
        } else {
            ""
        })
        .with_initialization_script(include_str!("runtime_bootstrap.js"))
        .with_ipc_handler(move |request| {
            if let Some(payload) = request.body().strip_prefix("fig-latency:") {
                crate::latency::webview_batch(payload);
                return;
            }
            let parts: Vec<_> = request.body().split(':').collect();
            let event = match parts.as_slice() {
                ["fig-runtime", "hello", nonce] if nonce.len() <= 100 => Some(WindowEvent::RuntimeHello {
                    epoch: ipc.page_epoch.current(),
                    nonce: (*nonce).into(),
                }),
                ["fig-runtime", "fatal", epoch] => epoch.parse().ok().map(WindowEvent::RuntimeFatal),
                ["fig-runtime", "ready", epoch] => epoch.parse().ok().map(WindowEvent::RuntimeReady),
                ["fig-runtime", kind @ ("ack" | "nack"), epoch, id] => {
                    epoch
                        .parse()
                        .ok()
                        .zip(id.parse().ok())
                        .map(|(epoch, id)| WindowEvent::RuntimeAck {
                            epoch,
                            id,
                            success: *kind == "ack",
                        })
                },
                _ => None,
            };
            if let Some(window_event) = event {
                ipc.proxy.send_event_or_log(Event::WindowEvent {
                    window_id: AUTOCOMPLETE_ID,
                    window_event,
                });
            }
        })
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, PageLoadEvent::Started) {
                navigation.page_epoch.renew();
                navigation.processes.cancel_window(&AUTOCOMPLETE_ID);
                navigation.proxy.send_event_or_log(Event::WindowEvent {
                    window_id: AUTOCOMPLETE_ID,
                    window_event: WindowEvent::RuntimeReset,
                });
            }
        })
        .with_on_web_content_process_terminate_handler(move || {
            state.page_epoch.renew();
            state.processes.cancel_window(&AUTOCOMPLETE_ID);
            state.proxy.send_event_or_log(Event::WindowEvent {
                window_id: AUTOCOMPLETE_ID,
                window_event: WindowEvent::RuntimeRecover,
            });
        })
}

pub fn install_focus_behavior(webview: &wry::WebView) -> anyhow::Result<()> {
    use wry::WebViewExtMacOS;
    super::objc_overrides::install(&webview.webview())
}
pub fn set_level(window: &tao::window::Window, level: i64) {
    use tao::platform::macos::WindowExtMacOS;
    let _mtm = objc2::MainThreadMarker::new().expect("AppKit window changes require main thread");
    // SAFETY: Tao owns the live NSWindow; the borrow never escapes this main-thread call.
    if let Some(window) = unsafe { window.ns_window().cast::<objc2_app_kit::NSWindow>().as_ref() } {
        window.setLevel(level as isize);
    }
}
