use fig_util::consts::PRODUCT_NAME;
use muda::{
    Menu,
    MenuEvent,
    MenuItemBuilder,
    PredefinedMenuItem,
};
use tao::event_loop::ControlFlow;
use tracing::{
    error,
    trace,
};
use tray_icon::{
    Icon,
    TrayIcon,
    TrayIconBuilder,
};

use crate::event::{
    Event,
    EventSender,
    WindowEvent,
};
use crate::{
    DASHBOARD_ID,
    EventLoopProxy,
};

pub fn handle_event(menu_event: &MenuEvent, proxy: &EventLoopProxy) {
    match &*menu_event.id().0 {
        "quit" => {
            proxy.send_event_or_log(Event::ControlFlow(ControlFlow::Exit));
        },
        "settings" => {
            proxy.send_event_or_log(Event::WindowEvent {
                window_id: DASHBOARD_ID.clone(),
                window_event: WindowEvent::Batch(vec![
                    WindowEvent::NavigateRelative {
                        path: "/autocomplete".into(),
                    },
                    WindowEvent::Show,
                ]),
            });
        },
        id => {
            trace!(?id, "Unhandled tray event");
        },
    }
}

pub fn build_tray() -> tray_icon::Result<TrayIcon> {
    TrayIconBuilder::new()
        .with_icon_templated(get_icon())
        .with_menu(Box::new(get_context_menu()))
        .build()
}

fn get_icon() -> Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let bytes = include_bytes!("../icons/icon-monochrome.png").to_vec();
        let image = image::load_from_memory(&bytes)
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to open icon")
}

fn get_context_menu() -> Menu {
    let menu = Menu::new();

    let items: [&dyn muda::IsMenuItem; 5] = [
        &MenuItemBuilder::new().text("Settings").id("settings".into()).build(),
        &PredefinedMenuItem::separator(),
        &MenuItemBuilder::new()
            .text(format!("Version: {}", env!("CARGO_PKG_VERSION")))
            .enabled(false)
            .build(),
        &PredefinedMenuItem::separator(),
        &MenuItemBuilder::new()
            .text(format!("Quit {PRODUCT_NAME}"))
            .id("quit".into())
            .build(),
    ];

    if let Err(err) = menu.append_items(&items) {
        error!(%err, "Failed to build the tray menu");
    }

    menu
}
