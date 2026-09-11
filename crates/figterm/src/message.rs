use std::collections::HashMap;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::atomic::Ordering;
use std::sync::{
    LazyLock,
    Mutex,
};
use std::time::{
    Duration,
    SystemTime,
};

use alacritty_terminal::Term;
use alacritty_terminal::term::ShellState;
use anyhow::Result;
use fig_proto::fig::{
    EnvironmentVariable,
    RunProcessResponse,
};
use fig_proto::figterm::figterm_request_message::Request as FigtermRequest;
use fig_proto::figterm::figterm_response_message::Response as FigtermResponse;
use fig_proto::figterm::intercept_request::{
    InterceptCommand,
    SetFigjsIntercepts,
    SetFigjsVisible,
};
use fig_proto::figterm::{
    self,
    FigtermRequestMessage,
    FigtermResponseMessage,
};
use fig_proto::remote::{
    Clientbound,
    Hostbound,
    clientbound,
    hostbound,
};
use fig_util::env_var::PROCESS_LAUNCHED_BY_Q;
use flume::Sender;
use tokio::process::Command;
use tracing::{
    debug,
    error,
    trace,
    warn,
};

use crate::event_handler::EventHandler;
use crate::interceptor::KeyInterceptor;
use crate::pty::AsyncMasterPty;
use crate::{
    CONTEXT_DIRTY,
    EXPECTED_BUFFER,
    INSERTION_LOCKED_AT,
    LockIgnorePoison as _,
    RwLockIgnorePoison as _,
    SHELL_ALIAS,
    SHELL_ENVIRONMENT_VARIABLES,
    shell_state_to_context,
};

fn working_directory(path: Option<&str>, shell_state: &ShellState) -> PathBuf {
    let map_dir = |path: PathBuf| match path.canonicalize() {
        Ok(path) if path.is_dir() => Some(path),
        Ok(path) => {
            warn!(?path, "not a directory");
            None
        },
        Err(err) => {
            warn!(?path, %err, "failed to canonicalize path");
            None
        },
    };

    path.map(PathBuf::from)
        .and_then(map_dir)
        .or_else(|| {
            shell_state
                .get_context()
                .current_working_directory
                .clone()
                .and_then(map_dir)
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Resolved `which` lookups, keyed by `(executable, PATH, cwd)`.
///
/// Autocomplete generators run a command per request, and `which_in` walks every `PATH` entry
/// (54 of them here) with a `stat` each. A cached hit is revalidated with a single `is_file`, so a
/// binary that has been moved or removed still falls through to a fresh lookup; misses are never
/// cached, so a newly installed binary is found the first time it is asked for.
type WhichKey = (PathBuf, String, PathBuf);

static WHICH_CACHE: LazyLock<Mutex<HashMap<WhichKey, PathBuf>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn which_cached(executable: &Path, path: Option<&str>, working_directory: &Path) -> Option<PathBuf> {
    let key: WhichKey = (
        executable.to_path_buf(),
        path.unwrap_or_default().to_owned(),
        working_directory.to_path_buf(),
    );

    if let Some(hit) = WHICH_CACHE.lock_ignore_poison().get(&key)
        && hit.is_file()
    {
        return Some(hit.clone());
    }

    let resolved = which::which_in(executable, path, working_directory).ok()?;
    WHICH_CACHE.lock_ignore_poison().insert(key, resolved.clone());
    Some(resolved)
}

fn create_command(executable: impl AsRef<Path>, working_directory: impl AsRef<Path>) -> Command {
    let env = (*SHELL_ENVIRONMENT_VARIABLES.lock_ignore_poison())
        .clone()
        .into_iter()
        .filter_map(|EnvironmentVariable { key, value }| value.map(|value| (key, value)))
        .collect::<Vec<_>>();

    let mut cmd = if executable.as_ref().is_absolute() {
        Command::new(executable.as_ref())
    } else {
        let path = env
            .iter()
            .find_map(|(key, value)| if key == "PATH" { Some(value.as_str()) } else { None });

        which_cached(executable.as_ref(), path, working_directory.as_ref())
            .map_or_else(|| Command::new(executable.as_ref()), Command::new)
    };

    cmd.current_dir(working_directory);

    {
        let pre_exec_fn = || {
            // Remove controlling terminal.
            nix::unistd::setsid()?;
            Ok(())
        };

        // SAFETY:
        // Operation: `std::os::unix::process::CommandExt::pre_exec`, an unsafe fn.
        // Contract (std): the closure runs in the child between `fork` and `exec`, so it may
        // only call async-signal-safe functions — the parent may be multi-threaded and hold
        // locks (notably the allocator's) that were taken by threads which do not exist in the
        // child.
        // Evidence, operation by operation inside `pre_exec_fn`:
        // - `nix::unistd::setsid()` is `setsid(2)`, which POSIX lists as async-signal-safe.
        // - The `?` on its `Result` converts `nix::Errno` into `io::Error` via
        //   `io::Error::from_raw_os_error`, which stores an integer and does not allocate.
        // - The closure captures nothing (`||`), so no destructor and no heap access is reachable through
        //   captures, and it cannot panic.
        // Postcondition: the child leaves its parent's session, i.e. it loses the controlling
        // terminal — which is the point of the call. On `Err`, `Command` reports the error to
        // the parent and `_exit`s the child.
        unsafe { cmd.pre_exec(pre_exec_fn) };
    }

    if !env.is_empty() {
        cmd.env_clear();
        cmd.envs(env);
    }

    cmd.env_remove("LS_COLORS");
    cmd.env_remove("CLICOLOR_FORCE");
    cmd.env_remove("CLICOLOR");
    cmd.env_remove("COLORTERM");
    cmd.envs([
        (PROCESS_LAUNCHED_BY_Q, "1"),
        ("HISTFILE", ""),
        ("HISTCONTROL", "ignoreboth"),
        ("TERM", "xterm-256color"),
        ("NO_COLOR", "1"),
    ]);

    cmd.kill_on_drop(true);

    cmd
}

/// Process the inner figterm request enum, shared between local and remote
pub async fn process_figterm_request(
    figterm_request: FigtermRequest,
    term: &Term<EventHandler>,
    pty_master: &mut Box<dyn AsyncMasterPty + Send + Sync>,
    key_interceptor: &mut KeyInterceptor,
) -> Result<Option<FigtermResponse>> {
    match figterm_request {
        FigtermRequest::InsertText(request) => {
            // If the shell is in prompt or a command is being executed, insert the text only
            // if the insert during command option is enabled.
            if term.shell_state().preexec && !request.insert_during_command() {
                return Ok(None);
            }

            let current_buffer = term.get_current_buffer().map(|buff| (buff.buffer, buff.cursor_idx));
            let mut insertion_string = String::new();
            if let Some((buffer, Some(position))) = current_buffer {
                if let Some(ref text_to_insert) = request.insertion {
                    trace!(?buffer, ?position);

                    // perform deletion
                    // if let Some(deletion) = command.deletion {
                    //     let deletion = deletion as usize;
                    //     buffer.drain(position - deletion..position);
                    // }
                    // // move cursor
                    // if let Some(offset) = command.offset {
                    //     position += offset as usize;
                    // }
                    // // split text by cursor
                    // let (left, right) = buffer.split_at(position);

                    INSERTION_LOCKED_AT.write_ignore_poison().replace(SystemTime::now());
                    let expected = format!("{buffer}{text_to_insert}");
                    trace!(?expected, "lock set, expected buffer");
                    *EXPECTED_BUFFER.lock_ignore_poison() = expected;
                }
                if let Some(ref insertion_buffer) = request.insertion_buffer
                    && buffer.ne(insertion_buffer)
                {
                    if buffer.starts_with(insertion_buffer) {
                        if let Some(len_diff) = buffer.len().checked_sub(insertion_buffer.len()) {
                            insertion_string.extend(std::iter::repeat_n('\x08', len_diff));
                        }
                    } else if insertion_buffer.starts_with(&buffer) {
                        insertion_string.push_str(&insertion_buffer[buffer.len()..]);
                    }
                }
            }
            insertion_string.push_str(&request.to_term_string());
            pty_master.write(insertion_string.as_bytes()).await?;
            Ok(None)
        },
        FigtermRequest::Intercept(request) => {
            match request.intercept_command {
                Some(InterceptCommand::SetFigjsIntercepts(SetFigjsIntercepts {
                    intercept_bound_keystrokes,
                    intercept_global_keystrokes,
                    actions,
                    override_actions,
                })) => {
                    key_interceptor.set_intercept_global(intercept_global_keystrokes);
                    key_interceptor.set_intercept(intercept_bound_keystrokes);
                    key_interceptor.set_actions(&actions, override_actions);
                },
                Some(InterceptCommand::SetFigjsVisible(SetFigjsVisible { visible })) => {
                    key_interceptor.set_window_visible(visible);
                },
                None => {},
            }

            Ok(None)
        },
        FigtermRequest::Diagnostics(_) => {
            let map_color = |color: &shell_color::VTermColor| -> figterm::TermColor {
                figterm::TermColor {
                    color: Some(match color {
                        shell_color::VTermColor::Rgb { red, green, blue } => {
                            figterm::term_color::Color::Rgb(figterm::term_color::Rgb {
                                r: *red as i32,
                                b: *blue as i32,
                                g: *green as i32,
                            })
                        },
                        shell_color::VTermColor::Indexed { idx } => figterm::term_color::Color::Indexed(*idx as u32),
                    }),
                }
            };

            let map_style = |style: &shell_color::SuggestionColor| -> figterm::TermStyle {
                figterm::TermStyle {
                    fg: style.fg().as_ref().map(map_color),
                    bg: style.bg().as_ref().map(map_color),
                }
            };

            let (edit_buffer, cursor_position) = term.get_current_buffer().map_or((None, None), |buf| {
                (Some(buf.buffer), buf.cursor_idx.and_then(|i| i.try_into().ok()))
            });

            let response = FigtermResponse::Diagnostics(figterm::DiagnosticsResponse {
                shell_context: Some(shell_state_to_context(term.shell_state())),
                zsh_autosuggestion_style: term.shell_state().zsh_autosuggestion_color.as_ref().map(map_style),
                edit_buffer,
                cursor_position,
            });

            Ok(Some(response))
        },
        FigtermRequest::UpdateShellContext(request) => {
            update_shell_context(request);
            Ok(None)
        },
    }
}

/// Replaces the environment/alias snapshot the hooks report, and marks the context dirty so the
/// next edit-buffer hook carries the new one. See [`crate::CONTEXT_DIRTY`].
fn update_shell_context(request: figterm::UpdateShellContextRequest) {
    if request.update_environment_variables {
        *SHELL_ENVIRONMENT_VARIABLES.lock_ignore_poison() = request.environment_variables;
    }
    if request.update_alias {
        *SHELL_ALIAS.lock_ignore_poison() = request.alias;
    }
    if request.update_environment_variables || request.update_alias {
        CONTEXT_DIRTY.store(true, Ordering::Release);
    }
}

/// Process a figterm request message
pub async fn process_figterm_message(
    figterm_request_message: FigtermRequestMessage,
    response_tx: Sender<FigtermResponseMessage>,
    term: &Term<EventHandler>,
    pty_master: &mut Box<dyn AsyncMasterPty + Send + Sync>,
    key_interceptor: &mut KeyInterceptor,
) -> Result<()> {
    match figterm_request_message.request {
        Some(request) => match process_figterm_request(request, term, pty_master, key_interceptor).await {
            Ok(Some(response)) => {
                let response_message = FigtermResponseMessage {
                    response: Some(response),
                };
                if let Err(err) = response_tx.send_async(response_message).await {
                    error!(%err, "Failed sending request response");
                }
            },
            Ok(None) => {},
            Err(err) => error!(%err, "Failed to process figterm message"),
        },
        None => warn!("Qterm message with no request"),
    }
    Ok(())
}

pub async fn process_remote_message(
    clientbound_message: Clientbound,
    response_tx: Sender<Hostbound>,
    term: &Term<EventHandler>,
    pty_master: &mut Box<dyn AsyncMasterPty + Send + Sync>,
    key_interceptor: &mut KeyInterceptor,
) -> Result<()> {
    use clientbound::request::Request;
    use hostbound::response::Response;

    match clientbound_message.packet {
        Some(clientbound::Packet::Request(request)) => {
            let nonce = request.nonce;
            let make_response = move |response: Response| -> Hostbound {
                Hostbound {
                    packet: Some(hostbound::Packet::Response(hostbound::Response {
                        response: Some(response),
                        nonce,
                    })),
                }
            };

            match request.request {
                // Neither of these produces a response: `process_figterm_request` returns `None`
                // for both, and `Hostbound.Response` no longer has a variant that could carry
                // one. `RunProcess` below builds its own.
                Some(Request::InsertText(request)) => {
                    process_figterm_request(FigtermRequest::InsertText(request), term, pty_master, key_interceptor)
                        .await?;
                },
                Some(Request::Intercept(request)) => {
                    process_figterm_request(FigtermRequest::Intercept(request), term, pty_master, key_interceptor)
                        .await?;
                },
                Some(Request::RunProcess(request)) => {
                    // TODO: we can infer shell as above for execute if no executable is provided.
                    let mut cmd = create_command(
                        &request.executable,
                        working_directory(request.working_directory.as_deref(), term.shell_state()),
                    );

                    cmd.args(request.arguments);
                    for var in request.env {
                        match var.value {
                            Some(value) => cmd.env(var.key, value),
                            None => cmd.env_remove(var.key),
                        };
                    }

                    tokio::spawn(async move {
                        debug!("running command");

                        let timeout_duration = request.timeout.map_or(Duration::from_secs(60), Into::into);
                        let command_timeout = tokio::time::timeout(timeout_duration, cmd.output());

                        let response = match command_timeout.await {
                            Ok(Ok(output)) => {
                                debug!("command successfully ran");
                                make_response(Response::RunProcess(RunProcessResponse {
                                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                                    exit_code: output.status.code().unwrap_or(0),
                                }))
                            },
                            Ok(Err(err)) => {
                                warn!(%err, executable = request.executable, "failed running executable");
                                make_response(Response::Error(format!(
                                    "failed running executable ({}): {err}",
                                    request.executable
                                )))
                            },
                            Err(err) => {
                                warn!(
                                    %err,
                                    executable = request.executable,
                                    timeout_ms =% timeout_duration.as_millis(),
                                    "timed out running executable"
                                );
                                make_response(Response::Error(format!(
                                    "timed out after {}ms running executable ({}): {err}",
                                    timeout_duration.as_millis(),
                                    request.executable,
                                )))
                            },
                        };

                        if let Err(err) = response_tx.send_async(response).await {
                            error!(%err, "Failed sending request response");
                        }
                    });
                },
                _ => warn!("unhandled request {request:?}"),
            }
        },
        Some(clientbound::Packet::Ping(())) => {
            let response = Hostbound {
                packet: Some(hostbound::Packet::Pong(())),
            };

            if let Err(err) = response_tx.send_async(response).await {
                error!(%err, "Failed sending request response");
            }
        },
        packet => warn!("unhandled packet {packet:?}"),
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_command() {
        let output = create_command("cargo", "/").output().await.unwrap();
        assert!(output.status.success());

        let output = create_command("echo", "/").arg("hello world").output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"hello world\n");
        assert_eq!(output.stderr, b"");

        let output = create_command("bash", "/")
            .args(["-c", "echo hello world 1 1>&2; echo hello world 2; exit 25"])
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(25));
        assert_eq!(output.stdout, b"hello world 2\n");
        assert_eq!(output.stderr, b"hello world 1\n");
    }

    /// The edit-buffer hook carries the environment and alias snapshot on the first hook after the
    /// shell pushed one, and nothing on the hooks after that; the prompt/preexec/postexec hooks
    /// always carry the full context. See `crate::CONTEXT_DIRTY`.
    #[test]
    fn edit_buffer_context_carries_the_environment_once_per_update() {
        use alacritty_terminal::term::ShellState;

        use crate::{
            shell_state_to_context,
            shell_state_to_edit_buffer_context,
        };

        let shell_state = ShellState::default();
        update_shell_context(figterm::UpdateShellContextRequest {
            update_environment_variables: true,
            environment_variables: vec![
                fig_proto::local::EnvironmentVariable {
                    key: "PATH".into(),
                    value: Some("/usr/bin".into()),
                },
                fig_proto::local::EnvironmentVariable {
                    key: "HOME".into(),
                    value: Some("/home/me".into()),
                },
            ],
            update_alias: true,
            alias: Some("gs='git status'\n".into()),
        });

        let (first, dirty) = shell_state_to_edit_buffer_context(&shell_state);
        assert!(dirty);
        assert_eq!(first.environment_variables.len(), 2);
        assert_eq!(first.alias.as_deref(), Some("gs='git status'\n"));

        for _ in 0..1_000 {
            let (next, dirty) = shell_state_to_edit_buffer_context(&shell_state);
            assert!(!dirty);
            assert!(next.environment_variables.is_empty());
            assert_eq!(next.alias, None);
            // Everything else stays in the lean hook.
            assert_eq!(next.figterm_version, first.figterm_version);
            assert_eq!(next.terminal, first.terminal);
        }

        // The per-command hooks are unaffected.
        let full = shell_state_to_context(&shell_state);
        assert_eq!(full.environment_variables.len(), 2);
        assert_eq!(full.alias.as_deref(), Some("gs='git status'\n"));

        // A fresh snapshot re-arms exactly one full hook.
        update_shell_context(figterm::UpdateShellContextRequest {
            update_environment_variables: true,
            environment_variables: vec![fig_proto::local::EnvironmentVariable {
                key: "PATH".into(),
                value: Some("/usr/local/bin".into()),
            }],
            update_alias: false,
            alias: None,
        });
        let (again, dirty) = shell_state_to_edit_buffer_context(&shell_state);
        assert!(dirty);
        assert_eq!(again.environment_variables.len(), 1);
        assert!(
            shell_state_to_edit_buffer_context(&shell_state)
                .0
                .environment_variables
                .is_empty()
        );
    }
}
