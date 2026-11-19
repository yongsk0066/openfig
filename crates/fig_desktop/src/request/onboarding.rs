use std::sync::Arc;

use fig_integrations::install::{
    InstallComponents,
    install,
};
use fig_proto::fig::{
    OnboardingAction,
    OnboardingRequest,
};
use tao::event_loop::ControlFlow;
use tokio::process::Command;
use tracing::error;

use super::{
    RequestResult,
    RequestResultImpl,
};
use crate::event::{
    Event,
    EventSender,
};

pub async fn onboarding(
    request: OnboardingRequest,
    proxy: &dyn EventSender,
    ctx: &Arc<fig_os_shim::Context>,
) -> RequestResult {
    match request.action() {
        OnboardingAction::InstallationScript => {
            install(InstallComponents::SHELL_INTEGRATIONS, ctx.env())
                .await
                .map_err(|e| e.to_string())?;
            RequestResult::success()
        },
        OnboardingAction::Uninstall => {
            use fig_integrations::install::uninstall;

            let result = match uninstall(InstallComponents::all(), Arc::clone(ctx)).await {
                Ok(_) => RequestResult::success(),
                Err(err) => RequestResult::error(err.to_string()),
            };

            proxy.send_event_or_log(Event::ControlFlow(ControlFlow::Exit));
            result
        },
        OnboardingAction::FinishOnboarding => {
            fig_settings::state::set_value("desktop.completedOnboarding", true).map_err(|e| e.to_string())?;
            RequestResult::success()
        },
        OnboardingAction::LaunchShellOnboarding => {
            fig_settings::state::set_value("user.onboarding", false).ok();

            let home = fig_util::directories::home_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));

            if let Err(err) = Command::new("open")
                .args(["-b", "com.apple.Terminal"])
                .arg(home)
                .spawn()
            {
                error!(%err, "Failed to open onboarding");
                return RequestResult::error("Failed to open onboarding");
            }

            RequestResult::success()
        },
        OnboardingAction::PromptForAccessibilityPermission => {
            use crate::local_ipc::{
                LocalResponse,
                commands,
            };
            let res = commands::prompt_for_accessibility_permission(ctx.env())
                .await
                .unwrap_or_else(|e| e);
            match res {
                LocalResponse::Success(_) => RequestResult::success(),
                LocalResponse::Error {
                    message: Some(message), ..
                } => RequestResult::error(message),
                _ => RequestResult::error("Failed to prompt for accessibility permissions"),
            }
        },
        OnboardingAction::CloseAccessibilityPromptWindow | OnboardingAction::RequestRestart => {
            RequestResult::error("Unimplemented")
        },
    }
}
