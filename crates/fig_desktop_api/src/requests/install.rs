mod ghostty;
use std::fmt::Display;

use fig_os_shim::Env;
use fig_proto::fig::install_response::{
    InstallationStatus,
    Response,
};
use fig_proto::fig::result::Result as ProtoResultEnum;
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    InstallAction,
    InstallComponent,
    InstallRequest,
    InstallResponse,
    Result as ProtoResult,
};

use super::RequestResult;

pub fn integration_result(result: Result<(), impl Display>) -> ServerOriginatedSubMessage {
    ServerOriginatedSubMessage::InstallResponse(InstallResponse {
        response: Some(Response::Result(match result {
            Ok(()) => ProtoResult {
                result: ProtoResultEnum::Ok.into(),
                error: None,
            },
            Err(err) => ProtoResult {
                result: ProtoResultEnum::Error.into(),
                error: Some(err.to_string()),
            },
        })),
    })
}

pub fn installation_status(status: InstallationStatus) -> ServerOriginatedSubMessage {
    ServerOriginatedSubMessage::InstallResponse(InstallResponse {
        response: Some(Response::InstallationStatus(status.into())),
    })
}

pub async fn install(request: InstallRequest, env: &Env) -> RequestResult {
    let (Ok(component), Ok(action)) = (
        InstallComponent::try_from(request.component),
        InstallAction::try_from(request.action),
    ) else {
        return Ok(Box::new(integration_result(Err("Unknown installation request"))));
    };
    let response = match (component, action) {
        (InstallComponent::GhosttyInputMethod, InstallAction::Status) => match ghostty::status().await {
            Ok(status) => installation_status(status),
            Err(err) => integration_result(Err(err)),
        },
        (InstallComponent::GhosttyInputMethod, InstallAction::Install) => integration_result(ghostty::enroll().await),
        (InstallComponent::GhosttyInputMethod, InstallAction::Uninstall) => {
            integration_result(Err("Disable Ghostty completions in Integrations."))
        },
        (InstallComponent::Dotfiles, InstallAction::Install) => integration_result(
            fig_integrations::install::install(fig_integrations::install::InstallComponents::SHELL_INTEGRATIONS, env)
                .await,
        ),
        (InstallComponent::Dotfiles, InstallAction::Uninstall) => {
            integration_result(fig_integrations::install::uninstall_shell_integrations(env).await)
        },
        (InstallComponent::Dotfiles, InstallAction::Status) => {
            match fig_integrations::install::shell_is_installed(env).await {
                Ok(true) => installation_status(InstallationStatus::Installed),
                Ok(false) => installation_status(InstallationStatus::NotInstalled),
                Err(err) => integration_result(Err(err)),
            }
        },
        (InstallComponent::Accessibility, InstallAction::Install) => {
            use macos_utils::accessibility::{
                accessibility_is_enabled,
                open_accessibility,
            };

            if !accessibility_is_enabled() && !open_accessibility() {
                return Ok(Box::new(integration_result(Err(
                    "Unable to open Accessibility settings",
                ))));
            }

            integration_result(Ok::<(), &str>(()))
        },
        (InstallComponent::Accessibility, InstallAction::Status) => {
            use macos_utils::accessibility::accessibility_is_enabled;

            ServerOriginatedSubMessage::InstallResponse(InstallResponse {
                response: Some(Response::InstallationStatus(if accessibility_is_enabled() {
                    InstallationStatus::Installed.into()
                } else {
                    InstallationStatus::NotInstalled.into()
                })),
            })
        },
        (InstallComponent::Accessibility, InstallAction::Uninstall) => integration_result(Ok::<(), &str>(())),
        (
            InstallComponent::LoginItem
            | InstallComponent::Ibus
            | InstallComponent::DesktopEntry
            | InstallComponent::AutostartEntry
            | InstallComponent::GnomeExtension,
            _,
        ) => integration_result(Err("Component is not available on macOS")),
    };

    RequestResult::Ok(Box::new(response))
}
