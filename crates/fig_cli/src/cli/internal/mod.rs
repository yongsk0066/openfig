pub mod local_state;
pub mod should_figterm_launch;

use std::collections::HashSet;
use std::io::{
    Write,
    stdout,
};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anstream::println;
use bytes::{
    Buf,
    BytesMut,
};
use clap::{
    ArgGroup,
    Args,
    Subcommand,
    ValueEnum,
};
use crossterm::style::Stylize;
use eyre::{
    Context,
    Result,
    bail,
};
use fig_integrations::install::InstallComponents;
use fig_ipc::local::send_hook_to_socket;
use fig_ipc::{
    BufferedUnixStream,
    SendMessage,
    SendRecvMessage,
};
use fig_os_shim::Context as OsContext;
use fig_proto::ReflectMessage;
use fig_proto::figterm::figterm_request_message::Request as FigtermRequest;
use fig_proto::figterm::{
    FigtermRequestMessage,
    UpdateShellContextRequest,
};
use fig_proto::hooks::new_event_hook;
use fig_proto::local::EnvironmentVariable;
use fig_proto::util::get_shell;
use fig_util::directories::figterm_socket_path;
use fig_util::env_var::QTERM_SESSION_ID;
use fig_util::{
    CLI_BINARY_NAME,
    directories,
};
use sysinfo::System;
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tokio::process::Command;
use tokio::select;
use tracing::{
    debug,
    error,
};

use crate::cli::installation::install_cli;

#[derive(Debug, Args, PartialEq, Eq)]
pub struct InstallArgs {
    /// Install only the shell integrations
    #[arg(long)]
    pub dotfiles: bool,
    /// Don't confirm automatic installation.
    #[arg(long)]
    pub no_confirm: bool,
    /// Force installation
    #[arg(long)]
    pub force: bool,
    /// Install globally
    #[arg(long)]
    pub global: bool,
}

impl From<InstallArgs> for InstallComponents {
    fn from(args: InstallArgs) -> Self {
        let InstallArgs { dotfiles, .. } = args;
        if dotfiles {
            let mut install_components = InstallComponents::empty();
            install_components.set(InstallComponents::SHELL_INTEGRATIONS, dotfiles);
            install_components
        } else {
            InstallComponents::all()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateComponent {
    Figterm,
    WebNotifications,
    Platform,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
#[command(hide = true, alias = "_")]
pub enum InternalSubcommand {
    /// Command that is run during the PreCmd section of the shell integrations.
    PreCmd {
        #[arg(long, allow_hyphen_values = true)]
        alias: Option<String>,
    },
    /// Change the local-state file
    LocalState(local_state::LocalStateArgs),
    /// Install the cli
    Install(InstallArgs),
    /// Uninstall the cli
    Uninstall {
        /// Uninstall only the shell integrations
        #[arg(long)]
        dotfiles: bool,
        /// Uninstall only the binary
        #[arg(long)]
        binary: bool,
    },
    GetShell,
    Hostname,
    /// Detects if Figterm should be launched
    ///
    /// Exit code:
    /// - 0 execute figterm
    /// - 1 dont execute figterm
    /// - 2 fallback to Q_TERM env
    ShouldFigtermLaunch,
    Event {
        /// Name of the event.
        #[arg(long)]
        name: String,
        /// Payload of the event as a JSON string.
        #[arg(long)]
        payload: Option<String>,
        /// Apps to send the event to.
        #[arg(long)]
        apps: Vec<String>,
    },
    SocketsDir,
    StreamFromSocket,
    FigtermSocketPath {
        session_id: String,
    },
    #[command(group(
        ArgGroup::new("target")
            .multiple(false)
            .required(true)
    ))]
    Ipc {
        #[arg(long, group = "target")]
        app: bool,
        #[arg(long, group = "target")]
        figterm: Option<String>,
        #[arg(long)]
        json: String,
        #[arg(long)]
        recv: bool,
    },
    UninstallForAllUsers,
    Uuidgen,
    DumpState {
        component: StateComponent,
    },
}

impl InternalSubcommand {
    pub async fn execute(self) -> Result<ExitCode> {
        let ctx = OsContext::new();
        match self {
            InternalSubcommand::Install(args) => {
                let no_confirm = args.no_confirm;
                let force = args.force;
                let global = args.global;
                install_cli(args.into(), no_confirm, force, global).await
            },
            InternalSubcommand::Uninstall { dotfiles, binary } => {
                let components = if dotfiles || binary {
                    let mut uninstall_components = InstallComponents::empty();
                    uninstall_components.set(InstallComponents::SHELL_INTEGRATIONS, dotfiles);
                    uninstall_components.set(InstallComponents::BINARY, binary);
                    uninstall_components
                } else {
                    InstallComponents::all()
                };
                if components.contains(InstallComponents::BINARY) {
                    if option_env!("Q_IS_PACKAGE_MANAGED").is_some() {
                        println!("Please uninstall using your package manager");
                    } else {
                        fig_integrations::install::uninstall(InstallComponents::BINARY, Arc::clone(&ctx)).await?;
                        println!("\n{}\n", "The binary was successfully uninstalled".bold());
                    }
                }

                let mut components = components;
                components.set(InstallComponents::BINARY, false);
                fig_integrations::install::uninstall(components, Arc::clone(&ctx)).await?;
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::PreCmd { alias } => Ok(pre_cmd(alias).await),
            InternalSubcommand::LocalState(local_state) => {
                local_state.execute().await?;
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::GetShell => match get_shell() {
                Ok(shell) => {
                    if write!(stdout(), "{shell}").is_ok() {
                        return Ok(ExitCode::SUCCESS);
                    }
                    Ok(ExitCode::FAILURE)
                },
                Err(_) => Ok(ExitCode::FAILURE),
            },
            InternalSubcommand::Hostname => {
                if let Some(hostname) = System::host_name()
                    && write!(stdout(), "{hostname}").is_ok()
                {
                    return Ok(ExitCode::SUCCESS);
                }
                Ok(ExitCode::FAILURE)
            },
            InternalSubcommand::ShouldFigtermLaunch => {
                Ok(should_figterm_launch::should_figterm_launch(&OsContext::new()))
            },
            InternalSubcommand::Event { payload, apps, name } => {
                let hook = new_event_hook(name, payload, apps);
                send_hook_to_socket(hook).await?;
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::Ipc {
                app,
                figterm,
                json,
                recv,
            } => {
                let message = fig_proto::FigMessage::json(serde_json::from_str::<serde_json::Value>(&json)?)?;

                let socket = if app {
                    directories::desktop_socket_path().expect("Failed to get socket path")
                } else if let Some(ref figterm) = figterm {
                    directories::figterm_socket_path(figterm).expect("Failed to get socket path")
                } else {
                    bail!("No destination for message");
                };

                let mut conn = BufferedUnixStream::connect(socket).await?;

                if recv {
                    macro_rules! recv {
                        ($abc:path) => {{
                            let response: Option<$abc> = conn.send_recv_message(message).await?;
                            match response {
                                Some(response) => {
                                    let message = response.transcode_to_dynamic();
                                    println!("{}", serde_json::to_string(&message)?)
                                },
                                None => bail!("Received EOF while waiting for response"),
                            }
                        }};
                    }

                    if app {
                        recv!(fig_proto::local::CommandResponse);
                    } else if figterm.is_some() {
                        recv!(fig_proto::figterm::FigtermResponseMessage);
                    }
                } else {
                    conn.send_message(message).await?;
                }

                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::SocketsDir => {
                writeln!(stdout(), "{}", directories::sockets_dir_utf8()?).ok();
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::FigtermSocketPath { session_id } => {
                writeln!(
                    stdout(),
                    "{}",
                    directories::figterm_socket_path(session_id)?.to_string_lossy()
                )
                .ok();
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::UninstallForAllUsers => {
                let out = Command::new("users").output().await?;
                let users = String::from_utf8_lossy(&out.stdout);

                let mut integrations_uninstalled = false;
                let mut data_dir_removed = false;

                let users = users
                    .split(' ')
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty())
                    .collect::<HashSet<_>>();

                for user in users {
                    println!("Uninstalling additional components for user: {}", user);

                    debug!(user, "Opening uninstall page");
                    let user_clone = user.to_string();
                    // Spawning a separate task since this will block unless and until the user
                    // answers the survey or closes their browser.
                    tokio::spawn(async move {
                        let user = user_clone;
                        match Command::new("runuser")
                            .args(["-u", &user, "--", CLI_BINARY_NAME, "_", "open-uninstall-page"])
                            .output()
                            .await
                        {
                            Ok(output) if output.status.success() => {
                                debug!(user, "Opened uninstall page");
                            },
                            Ok(output) => error!(user, ?output, "Failed to open uninstall page"),
                            Err(err) => error!(user, ?err, "Failed to open uninstall page"),
                        }
                    });

                    debug!(user, "Uninstalling integrations");
                    match Command::new("runuser")
                        .args([
                            "-u",
                            user,
                            "--",
                            CLI_BINARY_NAME,
                            "integrations",
                            "uninstall",
                            "--silent",
                            "all",
                        ])
                        .output()
                        .await
                    {
                        Ok(output) if output.status.success() => {
                            debug!(user, "Uninstalled integrations");
                            integrations_uninstalled = true;
                        },
                        Ok(output) => error!(user, ?output, "Failed to uninstall integrations"),
                        Err(err) => error!(user, ?err, "Failed to uninstall integrations"),
                    }

                    debug!(user, "Removing data dir");
                    match Command::new("runuser")
                        .args(["-u", user, "--", CLI_BINARY_NAME, "_", "remove-data-dir", "--force"])
                        .output()
                        .await
                    {
                        Ok(output) if output.status.success() => {
                            debug!(user, "Removed data dir");
                            data_dir_removed = true;
                        },
                        Ok(output) => error!(user, ?output, "Failed to remove data dir"),
                        Err(err) => error!(user, ?err, "Failed to remove data dir"),
                    }
                }

                // emit.await.ok();

                if !integrations_uninstalled {
                    bail!("Failed to uninstall completely: integrations were not uninstalled");
                }

                if !data_dir_removed {
                    bail!("Failed to uninstall completely: data directory was not removed");
                }

                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::StreamFromSocket => {
                let mut stdout = tokio::io::stdout();
                let mut stdin = tokio::io::stdin();

                let mut stdout_buf = BytesMut::with_capacity(1024);
                let mut stream_buf = BytesMut::with_capacity(1024);

                let socket = directories::local_remote_socket_path()?;
                while let Ok(mut stream) = BufferedUnixStream::connect_timeout(&socket, Duration::from_secs(5)).await {
                    loop {
                        select! {
                            n = stream.read_buf(&mut stdout_buf) => {
                                match n {
                                    Ok(0) | Err(_) => {
                                        break;
                                    }
                                    Ok(mut n) => {
                                        while !stdout_buf.is_empty() {
                                            let m = stdout.write(&stdout_buf[..n]).await?;
                                            stdout.flush().await?;
                                            stdout_buf.advance(m);
                                            n -= m;
                                        }
                                        stdout_buf.clear();
                                    }
                                }
                            }
                            n = stdin.read_buf(&mut stream_buf) => {
                                match n {
                                    Ok(0) | Err(_) => {
                                        break;
                                    }
                                    Ok(mut n) => {
                                        while !stream_buf.is_empty() {
                                            let m = stream.write(&stream_buf[..n]).await?;
                                            stream.flush().await?;
                                            stream_buf.advance(m);
                                            n -= m;
                                        }
                                        stream_buf.clear();
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::Uuidgen => {
                let _ = writeln!(stdout(), "{}", uuid::Uuid::new_v4());
                Ok(ExitCode::SUCCESS)
            },
            InternalSubcommand::DumpState { component } => {
                use fig_proto::local::dump_state_command::Type as StateCommandType;

                let state = fig_ipc::local::dump_state_command(match component {
                    StateComponent::Figterm => StateCommandType::DumpStateFigterm,
                    StateComponent::WebNotifications => StateCommandType::DumpStateWebNotifications,
                    StateComponent::Platform => StateCommandType::DumpStatePlatform,
                })
                .await
                .context("Failed to send dump state command")?;

                println!("{}", state.json);
                Ok(ExitCode::SUCCESS)
            },
        }
    }
}

pub async fn pre_cmd(alias: Option<String>) -> ExitCode {
    let Ok(session_id) = std::env::var(QTERM_SESSION_ID) else {
        return ExitCode::FAILURE;
    };

    match figterm_socket_path(&session_id) {
        Ok(figterm_path) => match fig_ipc::socket_connect(figterm_path).await {
            Ok(mut figterm_stream) => {
                let message = FigtermRequestMessage {
                    request: Some(FigtermRequest::UpdateShellContext(UpdateShellContextRequest {
                        update_environment_variables: true,
                        environment_variables: std::env::vars()
                            .map(|(key, value)| EnvironmentVariable {
                                key,
                                value: Some(value),
                            })
                            .collect(),
                        update_alias: true,
                        alias,
                    })),
                };
                if let Err(err) = figterm_stream.send_message(message).await {
                    error!(%err, %session_id, "Failed to send UpdateShellContext to Figterm");
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            },
            Err(err) => {
                error!(%err, %session_id, "Failed to connect to Figterm socket");
                ExitCode::FAILURE
            },
        },
        Err(err) => {
            error!(%err, %session_id, "Failed to get Figterm socket path");
            ExitCode::FAILURE
        },
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser, PartialEq, Eq)]
    pub struct MockCli {
        #[command(subcommand)]
        pub subcommand: InternalSubcommand,
    }

    #[test]
    fn parse_pre_cmd() {
        assert_eq!(MockCli::parse_from(["_", "pre-cmd"]), MockCli {
            subcommand: InternalSubcommand::PreCmd { alias: None }
        });

        let alias = format!("a='{CLI_BINARY_NAME} a'\nrd=rmdir");
        assert_eq!(MockCli::parse_from(["_", "pre-cmd", "--alias", &alias]), MockCli {
            subcommand: InternalSubcommand::PreCmd { alias: Some(alias) }
        });

        let hyphen_alias = "-='cd -'\n...=../..\nga='git add'";
        assert_eq!(
            MockCli::parse_from(["_", "pre-cmd", "--alias", hyphen_alias]),
            MockCli {
                subcommand: InternalSubcommand::PreCmd {
                    alias: Some(hyphen_alias.to_owned())
                }
            }
        );
    }
}
