use std::sync::Arc;

use fig_integrations::shell::ShellExt;
use fig_proto::fig::{
    OnboardingAction,
    OnboardingRequest,
};
use fig_util::Shell;
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

async fn finish_onboarding() {
    fig_settings::state::set_value("desktop.completedOnboarding", true).ok();
}

pub async fn onboarding(
    request: OnboardingRequest,
    proxy: &dyn EventSender,
    ctx: &Arc<fig_os_shim::Context>,
) -> RequestResult {
    match request.action() {
        OnboardingAction::InstallationScript => {
            let mut errs: Vec<String> = vec![];
            for shell in Shell::all() {
                match shell.get_shell_integrations(ctx.env()) {
                    Ok(integrations) => {
                        for integration in integrations {
                            if let Err(err) = integration.install().await {
                                errs.push(format!("{integration}: {err}"));
                            }
                        }
                    },
                    Err(err) => {
                        errs.push(format!("{shell}: {err}"));
                    },
                }
            }

            match &errs[..] {
                [] => RequestResult::success(),
                errs => RequestResult::error(errs.join("\n")),
            }
        },
        OnboardingAction::Uninstall => {
            use fig_integrations::install::{
                InstallComponents,
                uninstall,
            };

            let result = match uninstall(InstallComponents::all(), Arc::clone(ctx)).await {
                Ok(_) => RequestResult::success(),
                Err(err) => RequestResult::error(err.to_string()),
            };

            proxy.send_event_or_log(Event::ControlFlow(ControlFlow::Exit));
            result
        },
        OnboardingAction::FinishOnboarding => {
            finish_onboarding().await;
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
