use crate::application::ApplicationServices;
mod delivery;
pub mod menu;
pub mod notification;
pub(crate) mod page_epoch;
mod runtime;
pub mod url;
pub mod window;
pub mod window_id;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    Arc,
    LazyLock,
};

use ::url::Url;
use fig_desktop_api::init_script::javascript_init;
use fig_shell_ipc::session::ShellSessions;
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
use tray_icon::TrayIcon;
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
    NotificationBus,
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
use crate::shell_ipc::ShellHook;
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
    show_dashboard_after_launch: bool,
    fig_id_map: FigIdMap,
    window_id_map: WryIdMap,
    event_loop: EventLoop,
    proxy: EventLoopProxy,
    shell_sessions: Arc<ShellSessions>,
    platform_state: PlatformState,
    notifications_state: Arc<WebviewNotificationsState>,
    api_state: Arc<ApplicationServices>,
}

impl WebviewManager {
    pub fn new(show_dashboard_after_launch: bool) -> anyhow::Result<Self> {
        let main_thread = objc2::MainThreadMarker::new()
            .ok_or_else(|| anyhow::anyhow!("WebviewManager must be created on the main thread"))?;
        let mut event_loop = EventLoopBuilder::with_user_event().build();

        use tao::platform::macos::{
            ActivationPolicy,
            EventLoopExtMacOS,
        };
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        // Only an explicit dashboard presentation should take focus from the terminal.
        event_loop.set_activate_ignoring_other_apps(false);

        let proxy = EventLoopProxy::new(event_loop.create_proxy());

        let shell_sessions = Arc::new(ShellSessions::default());
        let platform_state = PlatformState::new(main_thread, ActivationPolicy::Accessory, proxy.clone())?;
        let notifications_state = Arc::new(WebviewNotificationsState::default());

        let api_state = Arc::new(ApplicationServices {
            settings_bus: Default::default(),
            icon_cache: Default::default(),
            page_epoch: Default::default(),
            io: fig_shell_ipc::process::Service::installed()?,
            processes: Default::default(),
            native_icons: fig_desktop::native::Icons::new().ok(),
            shell_sessions: shell_sessions.clone(),
            notifications_state: notifications_state.clone(),
            proxy: Box::new(proxy.clone()),
            position_window: Default::default(),
        });

        Ok(Self {
            show_dashboard_after_launch,
            fig_id_map: Default::default(),
            window_id_map: Default::default(),
            event_loop,
            proxy,
            shell_sessions,
            platform_state,
            notifications_state,
            api_state,
        })
    }

    fn insert_webview(
        &mut self,
        window: Window,
        window_id: WindowId,
        webview: WebView,
        context: WebContext,
        enabled: bool,
        url: Url,
    ) -> anyhow::Result<()> {
        let mut window_state = WindowState::new(
            window,
            window_id.clone(),
            webview,
            context,
            enabled,
            url,
            self.proxy.clone(),
        )?;
        window_state.page_epoch = self.api_state.page_epoch.clone();
        let webview_arc = Rc::new(window_state);
        self.fig_id_map.insert(window_id, webview_arc.clone());
        self.window_id_map.insert(webview_arc.window.id(), webview_arc);
        Ok(())
    }

    pub fn build_webview<T>(
        &mut self,
        window_id: WindowId,
        builder: impl Fn(Arc<ApplicationServices>, &mut WebContext, &EventLoop, T) -> anyhow::Result<(Window, WebView)>,
        options: T,
        enabled: bool,
        url_fn: impl Fn() -> Url,
    ) -> anyhow::Result<()> {
        let web_context_path = directories::fig_data_dir()?
            .join("webcontexts")
            .join(window_id.0.as_ref());
        let mut web_context = WebContext::new(Some(web_context_path));
        let (window, webview) = builder(Arc::clone(&self.api_state), &mut web_context, &self.event_loop, options)?;
        self.insert_webview(window, window_id, webview, web_context, enabled, url_fn())?;
        Ok(())
    }

    pub async fn run(self) -> anyhow::Result<()> {
        self.platform_state.handle(
            PlatformBoundEvent::Initialize,
            &self.event_loop,
            &self.fig_id_map,
            &self.notifications_state,
            &self.shell_sessions,
        )?;

        let mut tasks = tokio::task::JoinSet::new();
        {
            let shell_sessions = self.shell_sessions.clone();
            let notifications_state = self.notifications_state.clone();
            let event_loop = self.proxy.clone();
            tasks.spawn(async move {
                match local_ipc::start_local_ipc(shell_sessions, notifications_state, event_loop).await {
                    Ok(_) => (),
                    Err(err) => error!("Unable to start local ipc: {:?}", err),
                }
            });
        }

        let shell_hook = ShellHook {
            notifications_state: self.notifications_state.clone(),
            proxy: self.proxy.clone(),
        };

        let io = self.api_state.io.clone();
        // zsh shells that report through `fig.zsh`; `doctor` asks it whether it answers.
        {
            let shell_sessions = self.shell_sessions.clone();
            tasks.spawn(async move {
                let result = match fig_util::directories::hook_socket_path() {
                    Ok(path) => fig_shell_ipc::hook::start_hook_ipc(path, shell_sessions, shell_hook, io).await,
                    Err(err) => Err(err.into()),
                };
                if let Err(err) = result {
                    error!(%err, "Unable to start the zsh hook socket");
                }
            });
        }

        init_webview_notification_listeners(&mut tasks, self.proxy.clone(), &self.api_state.settings_bus);
        file_watcher::setup_listeners(
            &mut tasks,
            self.notifications_state.clone(),
            self.proxy.clone(),
            self.api_state.settings_bus.clone(),
        );

        // AppKit requires tray creation on the running main loop.
        let tray_visible = !fig_settings::settings::get_bool_or("app.hideMenubarIcon", false);
        let mut tray: Option<TrayIcon> = None;

        let menu_bar = menu_bar()?;
        menu_bar.init_for_nsapp();

        let proxy = self.proxy.clone();
        proxy
            .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::InitializePostRun))
            .map_err(|_closed| anyhow::anyhow!("Event loop closed during initialization"))?;

        {
            let menu_proxy = self.proxy.clone();
            MenuEvent::set_event_handler(Some(move |menu_event: MenuEvent| {
                info!(?menu_event, "Menu Event");
                menu::handle_event(&menu_event, &menu_proxy);
                tray::handle_event(&menu_event, &menu_proxy);
            }));
        }

        self.event_loop.run(move |event, window_target, control_flow| {
            *control_flow = ControlFlow::Wait;
            let _event_measure = crate::latency::Measure::new(event_stage(&event));
            trace!(?event, "Main loop event");

            match event {
                WryEvent::LoopDestroyed => tasks.abort_all(),
                WryEvent::NewEvents(StartCause::Init) => {
                    info!("{PRODUCT_NAME} has started");
                    if self.show_dashboard_after_launch && !crate::login_item::launched_at_login() {
                        proxy.send_event_or_log(Event::WindowEvent { window_id: DASHBOARD_ID, window_event: WindowEvent::Show });
                    }
                    crate::login_item::apply();
                    match build_tray() {
                        Ok(icon) => {
                            if let Err(err) = icon.set_visible(tray_visible) {
                                error!(%err, "Failed to set tray visible");
                            }
                            tray = Some(icon);
                        },
                        Err(err) => error!(%err, "Failed to build the tray icon"),
                    }
                },
                WryEvent::WindowEvent { event, window_id, .. } => {
                    if let Some(window_state) = self.window_id_map.get(&window_id) {
                        window_state.invalidate_geometry_screen();
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
                    let event = match event {
                        Event::PageEvent { epoch, event } if epoch == self.api_state.page_epoch.current() => *event,
                        Event::PageEvent { .. } => return,
                        event => event,
                    };
                    match event {
                        Event::PageEvent { .. } => unreachable!("page events are wrapped only once"),
                        Event::EditBuffer(pending) => {
                            let batch = pending.lock().take();
                            if let Some(batch) = batch {
                                tracing::trace!(session = %batch.session, queue_us = batch.received.elapsed().as_micros(), "input queue latency");
                                let trace_revision = batch.deliveries.iter().find(|d| d.window == AUTOCOMPLETE_ID).and_then(|d| d.notification.context.as_ref().and_then(|c| c.input_revision));
                                let had_deliveries = !batch.deliveries.is_empty();
                                let mut delivered = false;
                                for delivery in batch.deliveries {
                                    if crate::latency::enabled() && delivery.window == AUTOCOMPLETE_ID {
                                        crate::latency::record("main-queue", &batch.session.to_string(), delivery.notification.context.as_ref().and_then(|c| c.input_revision), Some(batch.received.elapsed().as_secs_f64() * 1000.0), None);
                                    }
                                    tracing::trace!(session = %batch.session, epoch = self.api_state.page_epoch.current(), revision = ?delivery.notification.context.as_ref().and_then(|c| c.input_revision), queue_us = batch.received.elapsed().as_micros(), "input queue delivery");
                                    let current = self
                                        .notifications_state
                                        .subscriptions
                                        .get(&delivery.window)
                                        .is_some_and(|state| {
                                            state.is_current_edit_subscription(delivery.channel, delivery.generation)
                                        });
                                    if !current {
                                        continue;
                                    }
                                    delivered = true;
                                    if let Some(window) = self.fig_id_map.get(&delivery.window) {
                                        window.emit_edit_buffer(delivery.channel, delivery.notification);
                                    }
                                }
                                if had_deliveries && !delivered {
                                    return;
                                }
                                let caret_started = std::time::Instant::now();
                                if !batch.empty
                                    && let Err(err) = self.platform_state.handle(
                                        PlatformBoundEvent::EditBufferChanged,
                                        window_target,
                                        &self.fig_id_map,
                                        &self.notifications_state,
                            &self.shell_sessions,
                                    )
                                {
                                    debug!(%err, "Failed to handle edit buffer position update");
                                }
                                if crate::latency::enabled() {
                                    crate::latency::record("caret-update", &batch.session.to_string(), trace_revision, Some(caret_started.elapsed().as_secs_f64() * 1000.0), None);
                                }
                                if let Some(window) = self.fig_id_map.get(&AUTOCOMPLETE_ID)
                                    && (window.enabled() || batch.empty)
                                {
                                    window.handle(
                                        if batch.empty {
                                            WindowEvent::Hide
                                        } else {
                                            WindowEvent::Show
                                        },
                                        &self.shell_sessions,
                                        &self.platform_state,
                                        &self.notifications_state,
                                        window_target,
                                    );
                                }
                            }
                        },
                        Event::WindowEvent {
                            window_id,
                            window_event,
                        } => match self.fig_id_map.get(&window_id) {
                            Some(window_state) => {
                                if window_state.enabled() || window_event.is_allowed_while_disabled() {
                                    window_state.handle(
                                        window_event,
                                        &self.shell_sessions,
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
                                // Late events for a removed window are discarded.
                                warn!("No window {window_id} available for event");
                                trace!(?window_event, "Event");
                            },
                        },
                        Event::WindowEventAll { window_event } => {
                            for window_state in self.window_id_map.values() {
                                if window_state.enabled() || window_event.is_allowed_while_disabled() {
                                    window_state.handle(
                                        window_event.clone(),
                                        &self.shell_sessions,
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
                        Event::InspectLoginItem(action,reply) => { let _ = reply.send(crate::login_item::inspect(action)); },
                        Event::ReconcileLoginItem => {
                            crate::login_item::apply();
                            proxy.send_event_or_log(Event::WindowEvent {window_id:DASHBOARD_ID,window_event:WindowEvent::Event {
                                event_name:"dashboard.loginItemChanged".into(),payload:None
                            }});
                        },
                        Event::SetTrayVisible(visible) => {
                            if let Some(tray) = tray.as_ref()
                                && let Err(err) = tray.set_visible(visible)
                            {
                                error!(%err, "Failed to set tray visible");
                            }
                        },
                        Event::PlatformBoundEvent(native_event) => {
                            if let Err(err) = self.platform_state.handle(
                                native_event,
                                window_target,
                                &self.fig_id_map,
                                &self.notifications_state,
                            &self.shell_sessions,
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
                // Tao owns the AppKit reopen callback; route it to the dashboard.
                WryEvent::Reopen { .. } => {
                    proxy.send_event_or_log(Event::WindowEvent {
                        window_id: DASHBOARD_ID,
                        window_event: WindowEvent::Show,
                    });
                },
                WryEvent::MainEventsCleared => {
                    for window in self.fig_id_map.values() {
                        window.flush_geometry(&self.platform_state);
                        window.invalidate_geometry_screen();
                    }
                },
                WryEvent::NewEvents(_) => {
                    for window in self.fig_id_map.values() { window.invalidate_geometry_screen(); }
                },
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
    state: Arc<ApplicationServices>,
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

    let mut url = self::url::dashboard();

    if show_onboarding {
        url.set_path(ONBOARDING_PATH);
    } else if let Some(page) = page {
        url.set_path(&page);
    }

    let webview_builder = WebViewBuilder::new_with_web_context(web_context)
        .with_focused(visible)
        .with_url(url.as_str())
        .with_devtools(true)
        .with_asynchronous_custom_protocol(
            "figresource".into(),
            utils::wrap_custom_protocol(
                "figresource::Dashboard",
                DashboardId,
                resource::handle::<resource::Dashboard>,
            ),
        )
        .with_asynchronous_custom_protocol(
            "api".into(),
            utils::wrap_custom_protocol("api", DashboardId, move |request, window_id| {
                api::handle(Arc::clone(&state), request, window_id)
            }),
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
    state: Arc<ApplicationServices>,
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

    let runtime_state = state.clone();
    // Completion keys belong to the terminal, including while the popup is clicked.
    window.set_focusable(false);
    let fig_icons = state.clone();
    let native_icons = state.clone();
    let webview_builder = WebViewBuilder::new_with_web_context(web_context)
        .with_focused(false)
        .with_url(self::url::autocomplete().as_str())
        .with_asynchronous_custom_protocol(
            "fig".into(),
            utils::wrap_custom_protocol("fig", AutocompleteId, move |request, id| {
                let state = fig_icons.clone();
                async move { icons::handle(request, id, state.native_icons.as_ref(), &state.icon_cache).await }
            }),
        )
        .with_asynchronous_custom_protocol(
            "icon".into(),
            utils::wrap_custom_protocol("icon", AutocompleteId, move |request, id| {
                let state = native_icons.clone();
                async move { icons::handle(request, id, state.native_icons.as_ref(), &state.icon_cache).await }
            }),
        )
        .with_asynchronous_custom_protocol(
            "spec".into(),
            utils::wrap_custom_protocol("spec", AutocompleteId, spec::handle),
        )
        .with_asynchronous_custom_protocol(
            "figresource".into(),
            utils::wrap_custom_protocol(
                "figresource::Autocomplete",
                AutocompleteId,
                resource::handle::<resource::Autocomplete>,
            ),
        )
        .with_asynchronous_custom_protocol(
            "api".into(),
            utils::wrap_custom_protocol("api", AutocompleteId, move |request, window_id| {
                api::handle(Arc::clone(&state), request, window_id)
            }),
        )
        .with_devtools(true)
        .with_transparent(true)
        .with_initialization_script(javascript_init(true))
        .with_navigation_handler(navigation_handler(AUTOCOMPLETE_ID, &[r"^localhost$"]))
        .with_clipboard(true)
        .with_hotkeys_zoom(true)
        .with_accept_first_mouse(true);

    let webview = crate::platform::webview_host::configure(webview_builder, runtime_state).build(&window)?;

    Ok((window, webview))
}

fn init_webview_notification_listeners(
    tasks: &mut tokio::task::JoinSet<()>,
    proxy: EventLoopProxy,
    bus: &NotificationBus,
) {
    macro_rules! watcher {
        ($type:ident, $name:expr, $on_update:expr) => {{
            let proxy = proxy.clone();
            let mut rx = bus.$type($name.into());
            tasks.spawn(async move {
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
        };};
    }

    watcher!(
        subscribe_settings,
        crate::login_item::SETTING,
        |_notification: JsonNotification, proxy: &EventLoopProxy| {
            proxy.send_event_or_log(Event::ReconcileLoginItem);
        }
    );

    watcher!(
        subscribe_settings,
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
        subscribe_settings,
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
        subscribe_settings,
        "app.hideMenubarIcon",
        |notification: JsonNotification, proxy: &EventLoopProxy| {
            let enabled = !notification.as_bool().unwrap_or(false);
            debug!(%enabled, "Tray icon");
            proxy.send_event_or_log(Event::SetTrayVisible(enabled));
        }
    );

    watcher!(
        subscribe_settings,
        "developer.dashboard.host",
        |_notification: JsonNotification, proxy: &EventLoopProxy| {
            let url = self::url::dashboard();
            debug!(%url, "Dashboard host");
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: DASHBOARD_ID,
                window_event: WindowEvent::NavigateAbsolute { url },
            });
        }
    );

    watcher!(
        subscribe_settings,
        "developer.autocomplete.host",
        |_notification: JsonNotification, proxy: &EventLoopProxy| {
            let url = self::url::autocomplete();
            debug!(%url, "Autocomplete host");
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: AUTOCOMPLETE_ID,
                window_event: WindowEvent::NavigateAbsolute { url },
            });
        }
    );
}

/// Static categories only: no event Debug output or terminal contents in diagnostics.
fn event_stage(event: &WryEvent<'_, Event>) -> &'static str {
    fn user(event: &Event) -> &'static str {
        match event {
            Event::PageEvent { event, .. } => user(event),
            Event::EditBuffer(_) => "loop-input",
            Event::PlatformBoundEvent(event) => match event {
                PlatformBoundEvent::ExternalWindowFocusChanged { .. } => "loop-focus",
                PlatformBoundEvent::FrameWatchTick => "loop-frame-watch",
                PlatformBoundEvent::CaretPositionUpdateRequested | PlatformBoundEvent::EditBufferChanged => {
                    "loop-caret"
                },
                PlatformBoundEvent::WindowDestroyed { .. } => "loop-destroyed",
                PlatformBoundEvent::WindowFocusValidated { .. } => "loop-focus-result",
                PlatformBoundEvent::FullscreenStateUpdated { .. } => "loop-fullscreen",
                PlatformBoundEvent::AccessibilityUpdated { .. } | PlatformBoundEvent::AccessibilityUpdateRequested => {
                    "loop-accessibility"
                },
                PlatformBoundEvent::AppWindowFocusChanged { .. } => "loop-own-focus",
                _ => "loop-platform",
            },
            Event::WindowEvent { .. } | Event::WindowEventAll { .. } => "loop-window",
            _ => "loop-other",
        }
    }
    match event {
        WryEvent::UserEvent(event) => user(event),
        WryEvent::MainEventsCleared => "loop-flush",
        WryEvent::WindowEvent { .. } => "loop-native-window",
        WryEvent::DeviceEvent { .. } => "loop-device",
        _ => "loop-lifecycle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Developer hosts must not bypass the exact localhost origin boundary.
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
