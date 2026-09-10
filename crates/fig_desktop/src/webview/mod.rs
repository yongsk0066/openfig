pub mod autocomplete;
pub mod dashboard;
pub mod menu;
pub mod notification;
pub mod window;
pub mod window_id;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    Arc,
    LazyLock,
};

use fig_desktop_api::init_script::javascript_init;
use fig_os_shim::Context;
use fig_remote_ipc::figterm::FigtermState;
use fig_util::consts::PRODUCT_NAME;
use fig_util::{
    URL_SCHEMA,
    directories,
};
use fnv::FnvBuildHasher;
use muda::MenuEvent;
use regex::RegexSet;
use tao::dpi::LogicalSize;
use tao::event::{
    Event as WryEvent,
    StartCause,
    WindowEvent as WryWindowEvent,
};
use tao::event_loop::{
    ControlFlow,
    EventLoopBuilder,
};
use tao::window::{
    Theme as TaoTheme,
    Window,
    WindowBuilder,
    WindowId as WryWindowId,
};
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};
use url::Url;
use window::WindowState;
use wry::{
    Theme as WryTheme,
    WebContext,
    WebView,
    WebViewBuilder,
};

use self::menu::menu_bar;
use self::notification::WebviewNotificationsState;
use self::window_id::DashboardId;
use crate::event::{
    Event,
    EventSender,
    ShowMessageNotification,
    WindowEvent,
};
use crate::notification_bus::{
    JsonNotification,
    NOTIFICATION_BUS,
};
use crate::platform::{
    PlatformBoundEvent,
    PlatformState,
};
use crate::protocol::{
    api,
    icons,
    resource,
    spec,
};
use crate::remote_ipc::RemoteHook;
use crate::tray::{
    self,
    build_tray,
};
use crate::webview::window_id::AutocompleteId;
pub use crate::webview::window_id::{
    AUTOCOMPLETE_ID,
    DASHBOARD_ID,
    WindowId,
};
use crate::{
    EventLoop,
    EventLoopProxy,
    InterceptState,
    file_watcher,
    local_ipc,
    utils,
};

pub const DASHBOARD_SIZE: LogicalSize<f64> = LogicalSize::new(960.0, 720.0);
pub const DASHBOARD_MINIMUM_SIZE: LogicalSize<f64> = LogicalSize::new(700.0, 480.0);

pub const AUTOCOMPLETE_WINDOW_TITLE: &str = "Fig Autocomplete";

pub const ONBOARDING_PATH: &str = "/";

fn map_theme(theme: &str) -> Option<WryTheme> {
    match theme {
        "dark" => Some(WryTheme::Dark),
        "light" => Some(WryTheme::Light),
        _ => None,
    }
}

fn to_tao_theme(theme: WryTheme) -> Option<TaoTheme> {
    match theme {
        WryTheme::Dark => Some(TaoTheme::Dark),
        WryTheme::Light => Some(TaoTheme::Light),
        WryTheme::Auto => None,
    }
}

pub static THEME: LazyLock<Option<WryTheme>> = LazyLock::new(|| {
    fig_settings::settings::get_string("app.theme")
        .ok()
        .flatten()
        .as_deref()
        .and_then(map_theme)
});

pub type FigIdMap = HashMap<WindowId, Rc<WindowState>, FnvBuildHasher>;
pub type WryIdMap = HashMap<WryWindowId, Rc<WindowState>, FnvBuildHasher>;

pub struct WebviewManager {
    fig_id_map: FigIdMap,
    window_id_map: WryIdMap,
    event_loop: EventLoop,
    figterm_state: Arc<FigtermState>,
    platform_state: Arc<PlatformState>,
    notifications_state: Arc<WebviewNotificationsState>,
    api_state: Arc<ApiState>,
}

/// Everything the `api://` protocol handler needs, captured by its `wrap_custom_protocol`
/// closure.
///
/// This used to be five `OnceLock` globals set in [`WebviewManager::new`], because the closure
/// signature only carried an `Arc<fig_os_shim::Context>` and a `WindowId`. They made
/// `protocol/api.rs` untestable — its own TODO said so.
pub struct ApiState {
    pub ctx: Arc<Context>,
    pub figterm_state: Arc<FigtermState>,
    pub intercept_state: Arc<InterceptState>,
    pub notifications_state: Arc<WebviewNotificationsState>,
    pub proxy: Box<dyn EventSender>,
    /// Serialises `positionWindowRequest`s.
    ///
    /// `wrap_custom_protocol` spawns a task per request, so without this every
    /// `positionWindow` call — including the `dryRun` probes the app fires while it decides
    /// whether the description pane fits — runs concurrently. The dead `window.ipc.postMessage`
    /// path used to funnel exactly these through a single task; nothing replaced that when
    /// `api://` took over, and `tokio`'s mutex hands the lock out in wait order, so this
    /// restores the same one-at-a-time behaviour.
    pub position_window: tokio::sync::Mutex<()>,
}

impl WebviewManager {
    #[allow(unused_variables)]
    #[allow(unused_mut)]
    pub fn new(context: Arc<Context>, visible: bool) -> Self {
        let mut event_loop = EventLoopBuilder::with_user_event().build();

        if !visible {
            use tao::platform::macos::{
                ActivationPolicy,
                EventLoopExtMacOS,
            };

            use crate::platform::ACTIVATION_POLICY;

            *ACTIVATION_POLICY.lock().unwrap() = ActivationPolicy::Accessory;
            event_loop.set_activation_policy(ActivationPolicy::Accessory);
        }

        let proxy = event_loop.create_proxy();

        let figterm_state = Arc::new(FigtermState::default());
        let platform_state = Arc::new(PlatformState::new(proxy.clone()));
        let notifications_state = Arc::new(WebviewNotificationsState::default());

        let api_state = Arc::new(ApiState {
            ctx: Arc::clone(&context),
            figterm_state: figterm_state.clone(),
            intercept_state: Arc::new(InterceptState::default()),
            notifications_state: notifications_state.clone(),
            proxy: Box::new(proxy),
            position_window: Default::default(),
        });

        Self {
            fig_id_map: Default::default(),
            window_id_map: Default::default(),
            event_loop,
            figterm_state,
            platform_state,
            notifications_state,
            api_state,
        }
    }

    fn insert_webview(
        &mut self,
        window: Window,
        window_id: WindowId,
        webview: WebView,
        context: WebContext,
        enabled: bool,
        url: Url,
    ) {
        let webview_arc = Rc::new(WindowState::new(
            window,
            window_id.clone(),
            webview,
            context,
            enabled,
            url,
        ));
        self.fig_id_map.insert(window_id, webview_arc.clone());
        self.window_id_map.insert(webview_arc.window.id(), webview_arc);
    }

    pub fn build_webview<T>(
        &mut self,
        window_id: WindowId,
        builder: impl Fn(Arc<ApiState>, &mut WebContext, &EventLoop, T) -> anyhow::Result<(Window, WebView)>,
        options: T,
        enabled: bool,
        url_fn: impl Fn() -> Url,
    ) -> anyhow::Result<()> {
        let web_context_path = directories::fig_data_dir()?
            .join("webcontexts")
            .join(window_id.0.as_ref());
        let mut web_context = WebContext::new(Some(web_context_path));
        let (window, webview) = builder(Arc::clone(&self.api_state), &mut web_context, &self.event_loop, options)?;
        self.insert_webview(window, window_id, webview, web_context, enabled, url_fn());
        Ok(())
    }

    #[allow(unused_mut)]
    pub async fn run(mut self) -> wry::Result<()> {
        self.platform_state
            .handle(
                PlatformBoundEvent::Initialize,
                &self.event_loop,
                &self.fig_id_map,
                &self.notifications_state,
            )
            .expect("Failed to initialize platform state");

        // TODO: implement
        // tokio::spawn(figterm::clean_figterm_cache(self.figterm_state.clone()));

        // Start the local ipc task, listens for requests to the desktop socket.
        {
            let platform_state = self.platform_state.clone();
            let figterm_state = self.figterm_state.clone();
            let notifications_state = self.notifications_state.clone();
            let event_loop = self.event_loop.create_proxy();
            tokio::spawn(async move {
                match local_ipc::start_local_ipc(platform_state, figterm_state, notifications_state, event_loop).await {
                    Ok(_) => (),
                    Err(err) => error!("Unable to start local ipc: {:?}", err),
                }
            });
        }

        tokio::spawn(fig_remote_ipc::remote::start_remote_ipc(
            fig_util::directories::local_remote_socket_path().unwrap(),
            self.figterm_state.clone(),
            RemoteHook {
                notifications_state: self.notifications_state.clone(),
                proxy: self.event_loop.create_proxy(),
                last_sent: Default::default(),
            },
        ));

        file_watcher::setup_listeners(self.notifications_state.clone(), self.event_loop.create_proxy()).await;

        init_webview_notification_listeners(self.event_loop.create_proxy()).await;

        let tray_visible = !fig_settings::settings::get_bool_or("app.hideMenubarIcon", false);
        let tray = build_tray(&self.event_loop, &self.figterm_state).await.unwrap();
        if let Err(err) = tray.set_visible(tray_visible) {
            error!(%err, "Failed to set tray visible");
        }

        let menu_bar = menu_bar();
        menu_bar.init_for_nsapp();

        let proxy = self.event_loop.create_proxy();
        proxy
            .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::InitializePostRun))
            .expect("Failed to send post init event");

        // Push menu clicks instead of polling for them. `MenuEvent::receiver().try_recv()` ran
        // on *every* iteration of the event loop — measured at 25,391 iterations over seven
        // minutes — and made menu handling depend on some other event arriving to wake the loop.
        {
            let menu_proxy = self.event_loop.create_proxy();
            MenuEvent::set_event_handler(Some(move |menu_event: MenuEvent| {
                info!(?menu_event, "Menu Event");
                menu::handle_event(&menu_event, &menu_proxy);
                tray::handle_event(&menu_event, &menu_proxy);
            }));
        }

        self.event_loop.run(move |event, window_target, control_flow| {
            *control_flow = ControlFlow::Wait;
            trace!(?event, "Main loop event");

            match event {
                WryEvent::NewEvents(StartCause::Init) => info!("{PRODUCT_NAME} has started"),
                WryEvent::WindowEvent { event, window_id, .. } => {
                    if let Some(window_state) = self.window_id_map.get(&window_id) {
                        match event {
                            WryWindowEvent::CloseRequested => {
                                // This is async so we need to pass 'visible' explicitly
                                window_state.window.set_visible(false);

                                if window_state.window_id == DASHBOARD_ID {
                                    proxy
                                        .send_event(Event::PlatformBoundEvent(
                                            PlatformBoundEvent::AppWindowFocusChanged {
                                                window_id: DASHBOARD_ID,
                                                focused: true, /* set to true, in order to update activation
                                                                * policy & remove from dock */
                                                fullscreen: false,
                                                visible: false,
                                            },
                                        ))
                                        .ok();
                                }
                            },
                            WryWindowEvent::ThemeChanged(theme) => window_state.set_theme(match theme {
                                TaoTheme::Light => Some(WryTheme::Light),
                                TaoTheme::Dark => Some(WryTheme::Dark),
                                _ => None,
                            }),
                            WryWindowEvent::Focused(focused) => {
                                if focused && window_state.window_id != AUTOCOMPLETE_ID {
                                    proxy.send_event_or_log(Event::WindowEvent {
                                        window_id: AUTOCOMPLETE_ID,
                                        window_event: WindowEvent::Hide,
                                    });
                                }

                                proxy.send_event_or_log(Event::PlatformBoundEvent(
                                    PlatformBoundEvent::AppWindowFocusChanged {
                                        window_id: window_state.window_id.clone(),
                                        focused,
                                        fullscreen: window_state.window.fullscreen().is_some(),
                                        visible: window_state.window.is_visible(),
                                    },
                                ));
                            },
                            _ => (),
                        }
                    }
                },
                WryEvent::UserEvent(event) => {
                    match event {
                        Event::WindowEvent {
                            window_id,
                            window_event,
                        } => match self.fig_id_map.get(&window_id) {
                            Some(window_state) => {
                                if window_state.enabled() || window_event.is_allowed_while_disabled() {
                                    window_state.handle(
                                        window_event,
                                        &self.figterm_state,
                                        &self.platform_state,
                                        &self.notifications_state,
                                        window_target,
                                    );
                                } else {
                                    trace!(
                                        window_id =% window_state.window_id,
                                        ?window_event,
                                        "Ignoring event for disabled window"
                                    );
                                }
                            },
                            None => {
                                // TODO(grant): figure out how to handle this gracefully
                                warn!("No window {window_id} available for event");
                                trace!(?window_event, "Event");
                            },
                        },
                        Event::WindowEventAll { window_event } => {
                            for window_state in self.window_id_map.values() {
                                if window_state.enabled() || window_event.is_allowed_while_disabled() {
                                    window_state.handle(
                                        window_event.clone(),
                                        &self.figterm_state,
                                        &self.platform_state,
                                        &self.notifications_state,
                                        window_target,
                                    );
                                } else {
                                    trace!(
                                        window_id =% window_state.window_id,
                                        ?window_event,
                                        "Ignoring event for disabled window"
                                    );
                                }
                            }
                        },
                        Event::ControlFlow(new_control_flow) => {
                            *control_flow = new_control_flow;
                        },
                        Event::ReloadAccessibility => {
                            let autocomplete_enabled =
                                !fig_settings::settings::get_bool_or("autocomplete.disable", false)
                                    && PlatformState::accessibility_is_enabled().unwrap_or(true);

                            proxy.send_event_or_log(Event::WindowEvent {
                                window_id: AUTOCOMPLETE_ID,
                                window_event: WindowEvent::SetEnabled(autocomplete_enabled),
                            });
                        },
                        Event::SetTrayVisible(visible) => {
                            if let Err(err) = tray.set_visible(visible) {
                                error!(%err, "Failed to set tray visible");
                            }
                        },
                        Event::PlatformBoundEvent(native_event) => {
                            if let Err(err) = self.platform_state.handle(
                                native_event,
                                window_target,
                                &self.fig_id_map,
                                &self.notifications_state,
                            ) {
                                debug!(%err, "Failed to handle native event");
                            }
                        },
                        Event::ShowMessageNotification(ShowMessageNotification {
                            title,
                            body,
                            parent,
                            buttons,
                            buttons_result,
                        }) => {
                            let mut dialog = rfd::AsyncMessageDialog::new().set_title(title).set_description(body);

                            if let Some(parent) = parent
                                && let Some(parent_window) = self.fig_id_map.get(&parent)
                            {
                                dialog = dialog.set_parent(&parent_window.window);
                            }

                            let dialog = match (buttons, buttons_result.as_ref()) {
                                (Some(buttons), Some(_)) => dialog.set_buttons(buttons),
                                _ => dialog,
                            };

                            tokio::spawn(async move {
                                let res = dialog.show().await;
                                if let Some(buttons_result) = buttons_result {
                                    buttons_result
                                        .send(res)
                                        .await
                                        .map_err(|err| error!(?err, "Failed to send dialog result"))
                                        .ok();
                                }
                            });
                        },
                    }
                },
                WryEvent::Opened { urls } => {
                    let mut events = Vec::new();
                    for url in urls {
                        if url.scheme() == URL_SCHEMA {
                            match url.host_str() {
                                Some("dashboard") => {
                                    events.push(WindowEvent::NavigateRelative {
                                        path: url.path().to_owned().into(),
                                    });
                                },
                                host => {
                                    error!(?host, "Invalid deep link");
                                },
                            }
                        } else {
                            error!(scheme = %url.scheme(), %url, "Invalid scheme");
                        }
                    }

                    if let Err(err) = proxy.send_event(Event::WindowEvent {
                        window_id: DASHBOARD_ID,
                        window_event: WindowEvent::Batch(events),
                    }) {
                        warn!(%err, "Error sending event");
                    }
                },
                // Dock icon clicked while the app has no visible window. tao raises this
                // directly; the app used to try to inject its own
                // `applicationShouldHandleReopen:hasVisibleWindows:` IMP into
                // `TaoAppDelegateParent`, which `class_addMethod` refused because tao defines
                // that selector itself — so it was a no-op with a `trace!` for a signal.
                WryEvent::Reopen { .. } => {
                    proxy.send_event_or_log(Event::WindowEvent {
                        window_id: DASHBOARD_ID,
                        window_event: WindowEvent::Show,
                    });
                },
                WryEvent::MainEventsCleared | WryEvent::NewEvents(StartCause::WaitCancelled { .. }) => {},
                // tao raises these from a global `NSEvent` monitor and offers no way to turn
                // them off; 2,090 of them in a seven-minute log, each one preparing a `trace!`
                // record nothing wants.
                WryEvent::DeviceEvent { .. } => {},
                event => trace!(?event, "Unhandled event"),
            }
        });
    }
}

fn navigation_handler<I, S>(window_id: WindowId, exprs: I) -> impl Fn(String) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let regex_set = RegexSet::new(exprs);

    if let Err(ref err) = regex_set {
        error!("Failed to compile regex: {err}");
    }

    move |url: String| match regex_set.as_ref().ok().and_then(|r| {
        Url::parse(&url)
            .ok()
            .and_then(|url| url.domain().map(|domain| r.is_match(domain)))
    }) {
        Some(true) => {
            trace!("{window_id} allowed url: {url}");
            true
        },
        Some(false) | None => {
            warn!("{window_id} denied url: {url}");
            false
        },
    }
}

pub struct DashboardOptions {
    pub show_onboarding: bool,
    pub visible: bool,
    pub page: Option<String>,
}

pub fn build_dashboard(
    state: Arc<ApiState>,
    web_context: &mut WebContext,
    event_loop: &EventLoop,
    DashboardOptions {
        show_onboarding,
        visible,
        page,
    }: DashboardOptions,
) -> anyhow::Result<(Window, WebView)> {
    let window = WindowBuilder::new()
        .with_title(PRODUCT_NAME)
        .with_inner_size(DASHBOARD_SIZE)
        .with_min_inner_size(DASHBOARD_MINIMUM_SIZE)
        .with_resizable(true)
        .with_maximizable(false)
        .with_visible(visible)
        .with_focused(visible)
        .with_always_on_top(false)
        .with_window_icon(Some(utils::icon()))
        .with_theme(THEME.and_then(to_tao_theme))
        .build(event_loop)?;

    let mut url = dashboard::url();

    if show_onboarding {
        url.set_path(ONBOARDING_PATH);
    } else if let Some(page) = page {
        url.set_path(&page);
    }

    let webview_builder = WebViewBuilder::new_with_web_context(web_context)
        .with_url(url.as_str())
        .with_devtools(true)
        .with_asynchronous_custom_protocol(
            "figresource".into(),
            utils::wrap_custom_protocol(
                Arc::clone(&state.ctx),
                "figresource::Dashboard",
                DashboardId,
                resource::handle::<resource::Dashboard>,
            ),
        )
        .with_asynchronous_custom_protocol(
            "api".into(),
            utils::wrap_custom_protocol(Arc::clone(&state), "api", DashboardId, api::handle),
        )
        .with_navigation_handler(navigation_handler(DASHBOARD_ID, &[r"^localhost$"]))
        .with_initialization_script(javascript_init(true))
        .with_clipboard(true)
        .with_hotkeys_zoom(true);

    let webview = webview_builder.build(&window)?;

    Ok((window, webview))
}

pub struct AutocompleteOptions;

pub fn build_autocomplete(
    state: Arc<ApiState>,
    web_context: &mut WebContext,
    event_loop: &EventLoop,
    _autocomplete_options: AutocompleteOptions,
) -> anyhow::Result<(Window, WebView)> {
    let mut window_builder = WindowBuilder::new()
        .with_title(AUTOCOMPLETE_WINDOW_TITLE)
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top(true)
        .with_focused(false)
        .with_window_icon(Some(utils::icon()))
        .with_inner_size(LogicalSize::new(1.0, 1.0))
        .with_theme(THEME.and_then(to_tao_theme));

    {
        use tao::platform::macos::WindowBuilderExtMacOS;
        window_builder = window_builder
            .with_resizable(false)
            .with_has_shadow(false)
            .with_visible(false);
    }

    let window = window_builder.build(event_loop)?;

    let webview_builder = WebViewBuilder::new_with_web_context(web_context)
        .with_url(autocomplete::url().as_str())
        .with_asynchronous_custom_protocol(
            "fig".into(),
            utils::wrap_custom_protocol(Arc::clone(&state.ctx), "fig", AutocompleteId, icons::handle),
        )
        .with_asynchronous_custom_protocol(
            "icon".into(),
            utils::wrap_custom_protocol(Arc::clone(&state.ctx), "icon", AutocompleteId, icons::handle),
        )
        .with_asynchronous_custom_protocol(
            "spec".into(),
            utils::wrap_custom_protocol(Arc::clone(&state.ctx), "spec", AutocompleteId, spec::handle),
        )
        .with_asynchronous_custom_protocol(
            "figresource".into(),
            utils::wrap_custom_protocol(
                Arc::clone(&state.ctx),
                "figresource::Autocomplete",
                AutocompleteId,
                resource::handle::<resource::Autocomplete>,
            ),
        )
        .with_asynchronous_custom_protocol(
            "api".into(),
            utils::wrap_custom_protocol(Arc::clone(&state), "api", AutocompleteId, api::handle),
        )
        .with_devtools(true)
        .with_transparent(true)
        .with_initialization_script(javascript_init(true))
        .with_navigation_handler(navigation_handler(AUTOCOMPLETE_ID, &[r"^localhost$"]))
        .with_clipboard(true)
        .with_hotkeys_zoom(true)
        .with_accept_first_mouse(true);

    let webview = webview_builder.build(&window)?;

    Ok((window, webview))
}

async fn init_webview_notification_listeners(proxy: EventLoopProxy) {
    #[allow(unused_macros)]
    macro_rules! watcher {
        ($type:ident, $name:expr, $on_update:expr) => {{
            paste::paste! {
                let proxy = proxy.clone();
                tokio::spawn(async move {
                    let mut rx = NOTIFICATION_BUS.[<subscribe_ $type>]($name.into());
                    loop {
                        let res = rx.recv().await;
                        match res {
                            Ok(val) => {
                                #[allow(clippy::redundant_closure_call)]
                                ($on_update)(val, &proxy);
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Notification bus '{}' lagged by {n} messages", $name);
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });
            }
        };};
    }

    watcher!(
        settings,
        "autocomplete.disable",
        |notification: JsonNotification, proxy: &EventLoopProxy| {
            let enabled =
                !notification.as_bool().unwrap_or(false) && PlatformState::accessibility_is_enabled().unwrap_or(true);
            debug!(%enabled, "Autocomplete");
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: AUTOCOMPLETE_ID,
                window_event: WindowEvent::SetEnabled(enabled),
            });
        }
    );

    watcher!(
        settings,
        "app.theme",
        |notification: JsonNotification, proxy: &EventLoopProxy| {
            let theme = notification.as_string().as_deref().and_then(map_theme);
            debug!(?theme, "Theme changed");
            proxy.send_event_or_log(Event::WindowEventAll {
                window_event: WindowEvent::SetTheme(theme),
            });
        }
    );

    watcher!(
        settings,
        "app.hideMenubarIcon",
        |notification: JsonNotification, proxy: &EventLoopProxy| {
            let enabled = !notification.as_bool().unwrap_or(false);
            debug!(%enabled, "Tray icon");
            proxy.send_event_or_log(Event::SetTrayVisible(enabled));
        }
    );

    watcher!(
        settings,
        "developer.dashboard.host",
        |_notification: JsonNotification, proxy: &EventLoopProxy| {
            let url = dashboard::url();
            debug!(%url, "Dashboard host");
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: DASHBOARD_ID,
                window_event: WindowEvent::NavigateAbsolute { url },
            });
        }
    );

    watcher!(
        settings,
        "developer.autocomplete.host",
        |_notification: JsonNotification, proxy: &EventLoopProxy| {
            let url = autocomplete::url();
            debug!(%url, "Autocomplete host");
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: AUTOCOMPLETE_ID,
                window_event: WindowEvent::NavigateAbsolute { url },
            });
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The autocomplete window used to allow `r"localhost$"` — no start anchor — while the
    /// dashboard used `r"^localhost$"`. `developer.autocomplete.host` can point the window at an
    /// arbitrary origin, so an unanchored pattern let `evil-localhost` through.
    #[test]
    fn only_exactly_localhost_is_allowed() {
        let handler = navigation_handler(AUTOCOMPLETE_ID, &[r"^localhost$"]);

        assert!(handler("http://localhost/".into()));
        assert!(handler("http://localhost:8080/autocomplete".into()));

        assert!(!handler("http://evil-localhost/".into()));
        assert!(!handler("http://x.notlocalhost/".into()));
        assert!(!handler("http://localhost.evil.com/".into()));
        assert!(!handler("https://example.com/".into()));
    }

    /// `Url::domain()` returns `None` for an IP literal, so a `^127\.0\.0\.1$` entry could never
    /// match and its removal changes nothing: the loopback IP was already denied.
    #[test]
    fn an_ip_literal_never_matched_the_allowlist() {
        let handler = navigation_handler(AUTOCOMPLETE_ID, &[r"^localhost$", r"^127\.0\.0\.1$"]);
        assert!(!handler("http://127.0.0.1/".into()));

        let handler = navigation_handler(AUTOCOMPLETE_ID, &[r"^localhost$"]);
        assert!(!handler("http://127.0.0.1/".into()));
    }
}
