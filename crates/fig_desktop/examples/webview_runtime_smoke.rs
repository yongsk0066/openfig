//! Real AppKit/WebKit regression for runtime overrides and asynchronous resize/KVO.
//! Run on macOS: cargo run --locked -p fig_desktop --example webview_runtime_smoke
use std::time::{
    Duration,
    Instant,
};

use tao::dpi::LogicalSize;
use tao::event::{
    Event,
    StartCause,
};
use tao::event_loop::{
    ControlFlow,
    EventLoopBuilder,
};
use tao::platform::macos::{
    ActivationPolicy,
    EventLoopExtMacOS,
};
use tao::window::WindowBuilder;
use wry::{
    WebViewBuilder,
    WebViewExtMacOS,
};

#[allow(dead_code)]
#[path = "../src/webview/delivery.rs"]
mod delivery;
#[path = "../src/platform/objc_overrides.rs"]
mod objc_overrides;

fn main() {
    let mut event_loop = EventLoopBuilder::<()>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let window = WindowBuilder::new()
        .with_title("Fig WebKit regression")
        .with_visible(false)
        .with_inner_size(LogicalSize::new(100.0, 100.0))
        .build(&event_loop)
        .unwrap();
    let messages = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let received = messages.clone();
    let html = "<html><body>resize regression<script>window.figRuntime.ready.then(() => window.ipc.postMessage('ready'))</script></body></html>";
    let webview = WebViewBuilder::new()
        .with_initialization_script(include_str!("../src/platform/runtime_bootstrap.js"))
        .with_ipc_handler(move |request| received.lock().unwrap().push(request.body().clone()))
        .with_html(html)
        .build(&window)
        .unwrap();
    // Diagnostic-only control: this reproduces the discarded isa-swapping approach.
    let reproduce_class_swap = std::env::args().any(|arg| arg == "--reproduce-class-swap");
    let mut started = None;
    let mut steps = 0;
    let mut delivery = delivery::Delivery::default();
    let mut acknowledgements = 0;
    let mut reloaded = false;
    let mut epoch = 1;
    let mut failures = 0;
    let mut fatals = 0;
    event_loop.run(move |event, _, flow| {
        if matches!(*flow, ControlFlow::ExitWithCode(_)) {
            return;
        }
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(20));
        match event {
            Event::NewEvents(StartCause::Init) => {
                let view = webview.webview();
                objc_overrides::install(&view).unwrap();
                if reproduce_class_swap {
                    use objc2::runtime::{
                        AnyObject,
                        ClassBuilder,
                    };
                    let object: &AnyObject = &view;
                    let class = ClassBuilder::new(c"FigRejectedClassSwap", object.class())
                        .unwrap()
                        .register();
                    // SAFETY: no ivars or methods are added; this satisfies objc2's layout
                    // contract but intentionally exposes the AppKit cached-property assertion.
                    // Never use this path in the installed app.
                    unsafe {
                        AnyObject::set_class(object, class);
                    }
                }
                started = Some(Instant::now());
            },
            Event::MainEventsCleared => {
                for message in std::mem::take(&mut *messages.lock().unwrap()) {
                    if let Some(nonce) = message.strip_prefix("fig-runtime:hello:") {
                        let nonce = serde_json::to_string(nonce).unwrap();
                        webview.evaluate_script("window.figRuntime.bind('old-document', '999');").unwrap();
                        webview
                            .evaluate_script(&format!("window.figRuntime.bind({nonce}, '{epoch}')"))
                            .unwrap();
                    } else if message == "ready" {
                        delivery.reset();
                        delivery.ready();
                        if reloaded {
                            webview
                                .evaluate_script(&delivery::acknowledged_script(
                                    epoch - 1,
                                    999,
                                    "window.ipc.postMessage('STALE');",
                                ))
                                .unwrap();
                        }
                        assert!(delivery.push(delivery::Frame::Script("window.figSmoke = 42;".into())));
                    } else if let Some(rest) = message.strip_prefix("fig-runtime:ack:") {
                        let (_, id) = rest.split_once(':').unwrap();
                        assert!(delivery.ack(id.parse().unwrap()));
                        acknowledgements += 1;
                        if reloaded { webview.evaluate_script("window.figRuntime.fatal();").unwrap(); }
                        if !reloaded {
                            delivery.push(delivery::Frame::Script(
                                "document.addEventListener('injected', () => { throw new Error('injected subscriber failure'); }, { once: true }); document.dispatchEvent(new Event('injected'));".into(),
                            ));
                        }
                    } else if let Some(value) = message.strip_prefix("fig-runtime:fatal:") {
                        assert_eq!(value.parse::<u64>().unwrap(), epoch);
                        fatals += 1;
                    } else if let Some(rest) = message.strip_prefix("fig-runtime:nack:") {
                        let (_, id) = rest.split_once(':').unwrap();
                        assert!(delivery.ack(id.parse().unwrap()));
                        failures += 1;
                        reloaded = true;
                        epoch += 1;
                        assert!(delivery.recover());
                        webview.load_html(html).unwrap();
                    } else {
                        panic!("unexpected IPC: {message}");
                    }
                }
                if let Some((id, delivery::Frame::Script(script))) = delivery.take() {
                    webview
                        .evaluate_script(&delivery::acknowledged_script(epoch, id, &script))
                        .unwrap();
                }
                if steps < 50 {
                    let side = if steps % 2 == 0 { 480.0 } else { 100.0 };
                    window.set_inner_size(LogicalSize::new(side, side / 2.0));
                    steps += 1;
                } else if started.is_some_and(|time| time.elapsed() > Duration::from_secs(3)) {
                    // Allow the queued AppKit size/KVO updates to drain before exiting.
                    assert_eq!(
                        acknowledgements, 2,
                        "initial and reloaded pages both acknowledge delivery"
                    );
                    assert_eq!(fatals, 1, "React fatal reports use the bound epoch");
                    assert_eq!(failures, 1, "failed processing must NACK, never ACK");
                    verify_window_query(&window);
                    println!("PASS: {steps} WKWebView resizes, IPC acknowledgements, fatal reporting and reload handshake");
                    *flow = ControlFlow::Exit;
                }
            },
            _ => {},
        }
    });
}

fn verify_window_query(window: &tao::window::Window) {
    use core_graphics::window::{
        copy_window_info,
        kCGWindowListOptionAll,
    };
    use macos_utils::window_server::UIElement;
    use tao::platform::macos::WindowExtMacOS;
    let _main = objc2::MainThreadMarker::new().unwrap();
    // SAFETY: the live Tao-owned NSWindow is borrowed only on this main-thread callback.
    let native = unsafe { window.ns_window().cast::<objc2_app_kit::NSWindow>().as_ref().unwrap() };
    let id = u32::try_from(native.windowNumber()).unwrap();
    let info = UIElement::window_info_for_id(id).expect("own window is described even while hidden");
    assert_eq!(info.owner_pid, u64::from(std::process::id()));
    assert_eq!(info.window_id, id);
    assert!(info.bounds.size.width > 0.0);
    assert!(UIElement::window_info_for_id(u32::MAX).is_none());
    println!("PASS: targeted window metadata, owner, hidden window and missing ID");
    if std::env::var_os("FIG_WINDOW_QUERY_BENCH").is_some() {
        let mut before = Vec::new();
        let mut after = Vec::new();
        for iteration in 0..60 {
            for direct in if iteration % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let start = Instant::now();
                if direct {
                    assert_eq!(UIElement::window_info_for_id(id).unwrap().window_id, id);
                } else {
                    assert!(!copy_window_info(kCGWindowListOptionAll, 0).unwrap().is_empty());
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if iteration >= 10 {
                    if direct {
                        after.push(elapsed);
                    } else {
                        before.push(elapsed);
                    }
                }
            }
        }
        for (label, mut values) in [("all-window-list", before), ("targeted-window", after)] {
            values.sort_by(f64::total_cmp);
            println!(
                "window-query {label} n=50 p50={:.3}ms p95={:.3}ms",
                (values[24] + values[25]) / 2.0,
                values[47]
            );
        }
    }
}
