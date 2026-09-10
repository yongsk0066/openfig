#[allow(unused_imports)]
use fig_util::consts::PRODUCT_NAME;
#[allow(unused_imports)]
use muda::{
    Menu,
    MenuEvent,
    Submenu,
};
use tao::event_loop::ControlFlow;

use crate::event::{
    Event,
    EventSender,
    WindowEvent,
};
use crate::{
    DASHBOARD_ID,
    EventLoopProxy,
};

const DASHBOARD_CLOSE: &str = "dashboard-close";
const APP_QUIT: &str = "app-quit";
const DASHBOARD_RELOAD: &str = "dashboard-reload";
const DASHBOARD_BACK: &str = "dashboard-back";
const DASHBOARD_FORWARD: &str = "dashboard-forward";

pub fn menu_bar() -> Menu {
    use muda::{
        MenuItemBuilder,
        PredefinedMenuItem,
        Submenu,
    };

    let menu_bar = Menu::new();

    let app_submenu = Submenu::new(PRODUCT_NAME, true);
    app_submenu
        .append_items(&[
            &MenuItemBuilder::new()
                .text("Backward")
                .id(DASHBOARD_BACK.into())
                .enabled(true)
                .accelerator(Some("super+["))
                .unwrap()
                .build(),
            &MenuItemBuilder::new()
                .text("Forward")
                .id(DASHBOARD_FORWARD.into())
                .enabled(true)
                .accelerator(Some("super+]"))
                .unwrap()
                .build(),
            &MenuItemBuilder::new()
                .text("Reload")
                .id(DASHBOARD_RELOAD.into())
                .enabled(true)
                .accelerator(Some("super+r"))
                .unwrap()
                .build(),
            &MenuItemBuilder::new()
                .text("Close Window")
                .id(DASHBOARD_CLOSE.into())
                .enabled(true)
                .accelerator(Some("super+w"))
                .unwrap()
                .build(),
            &MenuItemBuilder::new()
                .text(format!("Quit {PRODUCT_NAME}"))
                .id(APP_QUIT.into())
                .enabled(true)
                .accelerator(Some("super+q"))
                .unwrap()
                .build(),
        ])
        .unwrap();

    menu_bar.append(&app_submenu).unwrap();

    let edit_submenu = Submenu::new("Edit", true);
    edit_submenu
        .append_items(&[
            &PredefinedMenuItem::undo(Some("Undo")),
            &PredefinedMenuItem::redo(Some("Redo")),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::copy(Some("Copy")),
            &PredefinedMenuItem::paste(Some("Paste")),
            &PredefinedMenuItem::cut(Some("Cut")),
            &PredefinedMenuItem::select_all(Some("Select All")),
        ])
        .unwrap();

    menu_bar.append(&edit_submenu).unwrap();

    let window_submenu = Submenu::new("Window", true);
    window_submenu
        .append_items(&[&PredefinedMenuItem::minimize(Some("Minimize"))])
        .unwrap();
    menu_bar.append(&window_submenu).unwrap();

    let help_submenu = Submenu::new("Help", true);
    menu_bar.append(&help_submenu).unwrap();

    menu_bar
}

pub fn handle_event(menu_event: &MenuEvent, proxy: &EventLoopProxy) {
    match &menu_event.id().0 {
        // "Close Window" and "Quit" shared one id, so ⌘Q only hid the dashboard. With
        // `LSUIElement = true` there is no Dock icon, which left the tray, `q quit` and
        // `launchctl` as the only ways to actually stop the app.
        menu_id if menu_id == DASHBOARD_CLOSE => proxy.send_event_or_log(Event::WindowEvent {
            window_id: DASHBOARD_ID,
            window_event: WindowEvent::Hide,
        }),
        menu_id if menu_id == APP_QUIT => proxy.send_event_or_log(Event::ControlFlow(ControlFlow::Exit)),
        menu_id if menu_id == DASHBOARD_RELOAD => proxy.send_event_or_log(Event::WindowEvent {
            window_id: DASHBOARD_ID,
            window_event: WindowEvent::Reload,
        }),
        menu_id if menu_id == DASHBOARD_BACK => proxy.send_event_or_log(Event::WindowEvent {
            window_id: DASHBOARD_ID,
            window_event: WindowEvent::NavigateBack,
        }),
        menu_id if menu_id == DASHBOARD_FORWARD => proxy.send_event_or_log(Event::WindowEvent {
            window_id: DASHBOARD_ID,
            window_event: WindowEvent::NavigateForward,
        }),
        _ => (),
    }
}
