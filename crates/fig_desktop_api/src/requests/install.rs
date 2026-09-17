use std::fmt::Display;

use anstream::adapter::strip_str;
use fig_integrations::shell::ShellExt;
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
use fig_util::Shell;

use super::RequestResult;

fn integration_result(result: Result<(), impl Display>) -> ServerOriginatedSubMessage {
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

pub async fn install(request: InstallRequest, env: &Env) -> RequestResult {
    let response = match (request.component(), request.action()) {
        (InstallComponent::Dotfiles, action) => {
            let mut errs: Vec<String> = vec![];
            for shell in Shell::all() {
                match shell.get_shell_integrations(env) {
                    Ok(integrations) => {
                        for integration in integrations {
                            let res = match action {
                                InstallAction::Install => integration.install().await,
                                InstallAction::Uninstall => integration.uninstall().await,
                                InstallAction::Status => integration.is_installed().await,
                            };

                            if let Err(err) = res {
                                errs.push(format!(
                                    "{integration}: {}",
                                    strip_str(&err.verbose_message().to_string())
                                ));
                            }
                        }
                    },
                    Err(err) => {
                        errs.push(format!("{shell}: {}", strip_str(&err.verbose_message().to_string())));
                    },
                }
            }

            match action {
                InstallAction::Install | InstallAction::Uninstall => integration_result(match &errs[..] {
                    [] => Ok(()),
                    errs => Err(errs.join("\n\n")),
                }),
                InstallAction::Status => ServerOriginatedSubMessage::InstallResponse(InstallResponse {
                    response: Some(Response::InstallationStatus(
                        if errs.is_empty() {
                            InstallationStatus::Installed
                        } else {
                            InstallationStatus::NotInstalled
                        }
                        .into(),
                    )),
                }),
            }
        },
        (InstallComponent::Accessibility, InstallAction::Install) => {
            use macos_utils::accessibility::{
                accessibility_is_enabled,
                open_accessibility,
            };

            if !accessibility_is_enabled() {
                open_accessibility();
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
            InstallComponent::Ibus
            | InstallComponent::DesktopEntry
            | InstallComponent::AutostartEntry
            | InstallComponent::GnomeExtension,
            _,
        ) => integration_result(Err("Component is not available on macOS")),
    };

    RequestResult::Ok(Box::new(response))
}
