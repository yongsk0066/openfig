mod figterm;
mod notifications;
mod onboarding;
mod process;
mod properties;
mod window;

use std::marker::PhantomData;
use std::sync::Arc;

use fig_desktop_api::handler::Wrapped;
#[allow(unused_imports)]
pub use fig_desktop_api::requests::{
    Error,
    RequestResult,
    RequestResultImpl,
};
use fig_os_shim::{
    ContextArcProvider,
    ContextProvider,
};
use fig_proto::fig::{
    DragWindowRequest,
    InsertTextRequest,
    NotificationRequest,
    OnboardingRequest,
    PositionWindowRequest,
    RunProcessRequest,
    UpdateApplicationPropertiesRequest,
    WindowFocusRequest,
};
use fig_remote_ipc::figterm::FigtermState;
use fig_settings::{
    Settings,
    SettingsProvider,
    State,
    StateProvider,
};

use crate::InterceptState;
use crate::event::EventSender;
use crate::webview::WindowId;
use crate::webview::notification::WebviewNotificationsState;

pub struct Context<'a> {
    pub figterm_state: &'a FigtermState,
    pub intercept_state: &'a InterceptState,
    pub notifications_state: &'a WebviewNotificationsState,
    pub proxy: &'a dyn EventSender,
    pub window_id: &'a WindowId,
    pub settings: &'a Settings,
    pub state: &'a State,
    pub ctx: Arc<fig_os_shim::Context>,
}

impl SettingsProvider for Context<'_> {
    fn settings(&self) -> &Settings {
        self.settings
    }
}

impl StateProvider for Context<'_> {
    fn state(&self) -> &State {
        self.state
    }
}

impl ContextProvider for Context<'_> {
    fn context(&self) -> &fig_os_shim::Context {
        &self.ctx
    }
}

impl ContextArcProvider for Context<'_> {
    fn context_arc(&self) -> Arc<fig_os_shim::Context> {
        Arc::clone(&self.ctx)
    }
}

#[derive(Default)]
pub struct EventHandler<'a> {
    _lifetime: PhantomData<&'a ()>,
}

#[async_trait::async_trait]
impl<'a> fig_desktop_api::handler::EventHandler for EventHandler<'a> {
    type Ctx = Context<'a>;

    async fn notification(&self, request: Wrapped<Self::Ctx, NotificationRequest>) -> RequestResult {
        notifications::handle_request(
            request.request,
            request.context.window_id.clone(),
            request.message_id,
            request.context.notifications_state,
        )
        .await
    }

    async fn insert_text(&self, request: Wrapped<Self::Ctx, InsertTextRequest>) -> RequestResult {
        figterm::insert_text(request.request, request.context.figterm_state).await
    }

    async fn position_window(&self, request: Wrapped<Self::Ctx, PositionWindowRequest>) -> RequestResult {
        window::position_window(
            request.request,
            request.context.window_id.clone(),
            request.context.figterm_state,
            request.context.proxy,
        )
        .await
    }

    async fn window_focus(&self, request: Wrapped<Self::Ctx, WindowFocusRequest>) -> RequestResult {
        window::focus(
            request.request,
            request.context.window_id.clone(),
            request.context.proxy,
        )
        .await
    }

    async fn drag_window(&self, request: Wrapped<Self::Ctx, DragWindowRequest>) -> RequestResult {
        window::drag(
            request.request,
            request.context.window_id.clone(),
            request.context.proxy,
        )
        .await
    }

    async fn onboarding(&self, request: Wrapped<Self::Ctx, OnboardingRequest>) -> RequestResult {
        onboarding::onboarding(request.request, request.context.proxy, &request.context).await
    }

    async fn run_process(&self, request: Wrapped<Self::Ctx, RunProcessRequest>) -> RequestResult {
        process::run(request.request, request.context.figterm_state).await
    }

    async fn update_application_properties(
        &self,
        request: Wrapped<Self::Ctx, UpdateApplicationPropertiesRequest>,
    ) -> RequestResult {
        properties::update(
            request.request,
            request.context.figterm_state,
            request.context.intercept_state,
        )
    }
}
