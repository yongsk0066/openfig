pub mod cli;
mod event_handler;
pub mod history;
pub mod input;
pub mod interceptor;
pub mod ipc;
pub mod logger;
mod message;
pub mod pty;
pub mod term;

use std::env;
use std::ffi::{
    CString,
    OsStr,
};
use std::sync::atomic::{
    AtomicBool,
    Ordering,
};
use std::sync::{
    LazyLock,
    Mutex,
    RwLock,
};
use std::time::{
    Duration,
    SystemTime,
};

use alacritty_terminal::Term;
use alacritty_terminal::ansi::Processor;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::{
    ShellState,
    SizeInfo,
    TextBuffer,
};
use anyhow::{
    Context as _,
    Result,
};
use bytes::BytesMut;
use clap::Parser;
use cli::Cli;
use fig_log::{
    LogArgs,
    initialize_logging,
};
use fig_os_shim::{
    Context,
    Env,
};
use fig_proto::local::{
    self,
    EnvironmentVariable,
    TerminalCursorCoordinates,
};
use fig_proto::remote::Hostbound;
use fig_proto::remote_hooks::{
    hook_to_message,
    new_edit_buffer_hook,
};
use fig_settings::state;
use fig_util::env_var::{
    Q_LOG_LEVEL,
    Q_SHELL,
    Q_TERM,
    QTERM_SESSION_ID,
};
use fig_util::{
    PRODUCT_NAME,
    PTY_BINARY_NAME,
    Shell,
    Terminal as FigTerminal,
    directories,
};
use flume::Sender;
use nix::unistd::execvp;
use portable_pty::PtySize;
use tokio::io::{
    self,
    AsyncWriteExt,
};
use tokio::sync::oneshot;
use tokio::{
    runtime,
    select,
};
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};

use crate::event_handler::EventHandler;
use crate::input::{
    InputEvent,
    KeyCode,
    KeyCodeEncodeModes,
    KeyboardEncoding,
    Modifiers,
};
use crate::interceptor::KeyInterceptor;
use crate::ipc::{
    spawn_figterm_ipc,
    spawn_remote_ipc,
};
use crate::message::{
    process_figterm_message,
    process_remote_message,
};
use crate::pty::unix::open_pty;
use crate::pty::{
    AsyncMasterPtyExt,
    CommandBuilder,
};
use crate::term::{
    SystemTerminal,
    Terminal,
};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const BUFFER_SIZE: usize = 16384;

static INSERTION_LOCKED_AT: RwLock<Option<SystemTime>> = RwLock::new(None);
static EXPECTED_BUFFER: Mutex<String> = Mutex::new(String::new());

static SHELL_ENVIRONMENT_VARIABLES: Mutex<Vec<EnvironmentVariable>> = Mutex::new(Vec::new());
static SHELL_ALIAS: Mutex<Option<String>> = Mutex::new(None);

/// Lock helpers that ignore poisoning.
///
/// All four statics above hold plain data that is fully overwritten by whoever takes the lock, so
/// a panic elsewhere cannot leave them in a state a later reader has to care about. Poisoning them
/// only converts one crash into a cascade of `unwrap` panics on the main loop, which SIGHUPs the
/// user's shell.
pub(crate) trait LockIgnorePoison {
    type Guard<'a>
    where
        Self: 'a;

    fn lock_ignore_poison(&self) -> Self::Guard<'_>;
}

pub(crate) trait RwLockIgnorePoison {
    type ReadGuard<'a>
    where
        Self: 'a;
    type WriteGuard<'a>
    where
        Self: 'a;

    fn read_ignore_poison(&self) -> Self::ReadGuard<'_>;
    fn write_ignore_poison(&self) -> Self::WriteGuard<'_>;
}

impl<T> LockIgnorePoison for Mutex<T> {
    type Guard<'a>
        = std::sync::MutexGuard<'a, T>
    where
        T: 'a;

    fn lock_ignore_poison(&self) -> Self::Guard<'_> {
        self.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl<T> RwLockIgnorePoison for RwLock<T> {
    type ReadGuard<'a>
        = std::sync::RwLockReadGuard<'a, T>
    where
        T: 'a;
    type WriteGuard<'a>
        = std::sync::RwLockWriteGuard<'a, T>
    where
        T: 'a;

    fn read_ignore_poison(&self) -> Self::ReadGuard<'_> {
        self.read().unwrap_or_else(|err| err.into_inner())
    }

    fn write_ignore_poison(&self) -> Self::WriteGuard<'_> {
        self.write().unwrap_or_else(|err| err.into_inner())
    }
}

static USER_ENABLED_SHELLS: LazyLock<Vec<String>> = LazyLock::new(|| {
    fig_settings::state::get("user.enabled-shells")
        .ok()
        .flatten()
        .unwrap_or_default()
});

static HOSTNAME: LazyLock<Option<String>> = LazyLock::new(sysinfo::System::host_name);

pub enum MainLoopEvent {
    Insert {
        insert: Vec<u8>,
        unlock: bool,
        bracketed: bool,
        execute: bool,
    },
    UnlockInterception,
    SetImmediateMode(bool),
    SetCsiU,
    UnsetCsiU,
}

/// The channel `EventHandler` pushes main-loop events onto.
///
/// It **must** stay unbounded. The only consumer of the receiver is the `select!` loop in
/// [`figterm_main`], and `EventHandler::send_event` is called from *inside* that loop: the
/// terminal parser invokes it while the loop is busy in its `master.read` arm. A bounded channel
/// therefore lets the loop deadlock against itself — flume's blocking `send()` parks the very
/// thread that is supposed to drain the channel. Each command cycle pushes two events (three with
/// csi-u enabled), and one 16 KiB read of a pasted script easily carries dozens of prompts, so the
/// old `bounded(16)` wedged the shell on a large paste.
fn main_loop_channel() -> (flume::Sender<MainLoopEvent>, flume::Receiver<MainLoopEvent>) {
    flume::unbounded()
}

/// The last `(buffer, cursor)` pair actually handed to the desktop.
///
/// `can_send_edit_buffer` runs after *every* read from the pty, not just after a keystroke, so a
/// command printing in the background re-sent a byte-identical edit buffer for each chunk of its
/// output. Cleared whenever the desktop (re)connects so the first hook of a session always goes
/// out.
static LAST_SENT_EDIT_BUFFER: Mutex<Option<(String, i64)>> = Mutex::new(None);

pub(crate) fn forget_last_sent_edit_buffer() {
    *LAST_SENT_EDIT_BUFFER.lock_ignore_poison() = None;
}

/// Set when the shell pushes a new environment/alias snapshot (`UpdateShellContextRequest`),
/// cleared by the next edit-buffer hook that carries one.
///
/// The environment (73 variables / ~3.8 KB here) and the alias table (~7.9 KB) are cloned into
/// every hook, and an edit-buffer hook goes out on every keystroke: ~13.5 KB per key, ~76x the
/// 178 B the hook itself needs. Both only change when the shell says so — once per prompt at
/// most — so the edit-buffer hook carries them only on the first hook after such a change and
/// sends `environment_variables: []` / `alias: None` otherwise.
///
/// Only `send_edit_buffer` consumes the flag. The prompt/preexec/postexec hooks always carry the
/// full context: they are the ones the desktop side seeds a session's context from, and they fire
/// at most once per command.
///
/// This half is useless on its own — a receiver that treats an empty `environment_variables` as
/// "the shell has no environment" would wipe the session. It must ship together with the desktop
/// side (`with_update` keeping the previous env when a hook carries none) and the JS guard in
/// `packages/autocomplete-app/src/state/index.ts`, because protobuf-es decodes an empty repeated
/// field as `[]`, which is truthy.
static CONTEXT_DIRTY: AtomicBool = AtomicBool::new(true);

/// The terminal emulator figterm is running inside.
///
/// Resolving this walks five levels of the process tree with a syscall at each step (plus building
/// a fresh `Env`), and it was being redone for *every* hook — i.e. on every keystroke. The answer
/// cannot change for the life of the process: figterm is `exec`ed by the shell the emulator
/// started, and neither `__CFBundleIdentifier`/`TERM_PROGRAM` nor our ancestry moves afterwards.
static PARENT_TERMINAL: LazyLock<Option<String>> =
    LazyLock::new(|| FigTerminal::parent_terminal(&Context::new()).map(|s| s.to_string()));

/// The full shell context, environment and aliases included.
///
/// Used by the prompt/preexec/postexec and intercepted-key hooks, which fire at most once per
/// command. The edit-buffer hook uses [`shell_state_to_edit_buffer_context`] instead.
fn shell_state_to_context(shell_state: &ShellState) -> local::ShellContext {
    shell_state_to_context_inner(shell_state, true)
}

/// The context for an edit-buffer hook: environment and aliases only on the first hook after the
/// shell pushed a new snapshot. See [`CONTEXT_DIRTY`].
fn shell_state_to_edit_buffer_context(shell_state: &ShellState) -> (local::ShellContext, bool) {
    let dirty = CONTEXT_DIRTY.swap(false, Ordering::AcqRel);
    (shell_state_to_context_inner(shell_state, dirty), dirty)
}

fn shell_state_to_context_inner(shell_state: &ShellState, include_environment: bool) -> local::ShellContext {
    let terminal = PARENT_TERMINAL.clone();

    local::ShellContext {
        pid: shell_state.local_context.pid,
        ttys: shell_state.local_context.tty.clone(),
        process_name: shell_state.local_context.shell.clone(),
        shell_path: shell_state
            .local_context
            .shell_path
            .clone()
            .map(|path| path.display().to_string()),
        wsl_distro: shell_state.local_context.wsl_distro.clone(),
        current_working_directory: shell_state
            .local_context
            .current_working_directory
            .clone()
            .map(|cwd| cwd.display().to_string()),
        session_id: shell_state.local_context.session_id.clone(),
        terminal,
        hostname: shell_state
            .local_context
            .username
            .as_deref()
            .and_then(|username| HOSTNAME.as_deref().map(|hostname| format!("{username}@{hostname}"))),
        environment_variables: if include_environment {
            SHELL_ENVIRONMENT_VARIABLES.lock_ignore_poison().clone()
        } else {
            Vec::new()
        },
        qterm_version: Some(env!("CARGO_PKG_VERSION").into()),
        preexec: Some(shell_state.preexec),
        osc_lock: Some(shell_state.osc_lock),
        alias: if include_environment {
            SHELL_ALIAS.lock_ignore_poison().clone()
        } else {
            None
        },
    }
}

/// The edit buffer that should be sent right now, or `None` if this is not a moment to send one.
///
/// This used to be a `bool` predicate whose caller then read the buffer a second time. The read is
/// a scan of the grid from the command cursor to the bottom, so while an insertion lock was
/// outstanding every send cost two of them.
///
/// The insertion-lock bookkeeping deliberately runs before any of the cheap gates: releasing the
/// lock is a side effect, and the main loop's 16 ms timer stays armed for as long as it is set.
fn can_send_edit_buffer<T>(term: &Term<T>) -> Option<TextBuffer>
where
    T: EventListener,
{
    // `None` = not read yet; `Some(None)` = read, and there was no buffer.
    let mut buffer: Option<Option<TextBuffer>> = None;

    let mut handle = INSERTION_LOCKED_AT.write_ignore_poison();
    let insertion_locked = match handle.as_ref() {
        Some(at) => {
            let lock_expired = at.elapsed().unwrap_or(Duration::ZERO) > Duration::from_millis(16);
            let should_unlock = lock_expired
                || buffer
                    .insert(term.get_current_buffer())
                    .as_ref()
                    .is_none_or(|buff| &buff.buffer == (&EXPECTED_BUFFER.lock_ignore_poison() as &String));
            if should_unlock {
                handle.take();
                if lock_expired {
                    trace!("insertion lock released because lock expired");
                } else {
                    trace!("insertion lock released because buffer looks like how we expect");
                }
                false
            } else {
                true
            }
        },
        None => false,
    };
    drop(handle);

    // Read the shell name once instead of once per candidate, and strip the `<shell> (figterm)`
    // suffix without allocating — we actually want to work with a nested figterm :)
    let shell = term.shell_state().get_context().shell.as_deref().map(|shell| {
        shell
            .strip_suffix(')')
            .and_then(|s| s.strip_suffix(PTY_BINARY_NAME))
            .and_then(|s| s.strip_suffix(" ("))
            .unwrap_or(shell)
    });
    // `dash` is not a `Shell` variant — the integration is never installed into it — but figterm
    // is routinely `exec`ed under `sh`/`dash` as a login shell, and the edit buffer works there,
    // so it stays as an explicit extra rather than being folded into `Shell::all()`.
    let shell_enabled = Shell::all()
        .iter()
        .map(Shell::as_str)
        .chain(std::iter::once("dash"))
        .chain(USER_ENABLED_SHELLS.iter().map(|s| s.as_str()))
        .any(|s| shell == Some(s));

    let preexec = term.shell_state().preexec;

    trace!(%shell_enabled, %preexec, %insertion_locked, "can_send_edit_buffer");

    if !shell_enabled || insertion_locked || preexec {
        return None;
    }

    // Same gates `send_edit_buffer` applies, checked before the grid scan rather than after it, so
    // a disabled or detached session pays nothing per read.
    if !*AUTOCOMPLETE_ENABLED || !ipc::desktop_connected() {
        return None;
    }

    match buffer {
        Some(buffer) => buffer,
        None => term.get_current_buffer(),
    }
}

const Q_DISABLE_AUTOCOMPLETE: &str = "Q_DISABLE_AUTOCOMPLETE";

fn autocomplete_enabled(env: &Env) -> bool {
    env.get_os(Q_DISABLE_AUTOCOMPLETE).is_none_or(|s| s.is_empty())
}

static AUTOCOMPLETE_ENABLED: LazyLock<bool> = LazyLock::new(|| autocomplete_enabled(&Env::new()));

async fn send_edit_buffer(
    shell_state: &ShellState,
    edit_buffer: TextBuffer,
    sender: &Sender<Hostbound>,
    cursor_coordinates: Option<TerminalCursorCoordinates>,
) -> Result<()> {
    if !*AUTOCOMPLETE_ENABLED {
        return Ok(());
    }

    // Nothing drains the outgoing channel while the desktop app is away, and everything queued is
    // thrown away by `outgoing_rx.drain()` at the next handshake, so a hook sent now is pure
    // memory growth: one per keystroke, for as long as the app is closed.
    if !ipc::desktop_connected() {
        trace!("Not sending edit buffer: desktop app is not connected");
        return Ok(());
    }

    let Some(cursor_idx) = edit_buffer.cursor_idx.and_then(|i| i.try_into().ok()) else {
        return Ok(());
    };

    debug!("edit_buffer: {edit_buffer:?}");
    trace!("buffer bytes: {:02X?}", edit_buffer.buffer.as_bytes());
    trace!("buffer chars: {:?}", edit_buffer.buffer.chars().collect::<Vec<_>>());

    {
        let mut last_sent = LAST_SENT_EDIT_BUFFER.lock_ignore_poison();
        if last_sent
            .as_ref()
            .is_some_and(|(buffer, cursor)| buffer == &edit_buffer.buffer && *cursor == cursor_idx)
        {
            trace!("Not sending edit buffer: unchanged since the last hook");
            return Ok(());
        }
        *last_sent = Some((edit_buffer.buffer.clone(), cursor_idx));
    }

    let (context, was_dirty) = shell_state_to_edit_buffer_context(shell_state);

    let edit_buffer_hook = new_edit_buffer_hook(Some(context), edit_buffer.buffer, cursor_idx, 0, cursor_coordinates);
    let message = hook_to_message(edit_buffer_hook);

    trace!("Sending: {message:?}");

    if let Err(err) = sender.send_async(message).await {
        // Nothing reached the desktop, so the next hook has to carry all of it again.
        if was_dirty {
            CONTEXT_DIRTY.store(true, Ordering::Release);
        }
        forget_last_sent_edit_buffer();
        return Err(err.into());
    }
    Ok(())
}

fn get_parent_shell() -> Result<String> {
    match env::var(Q_SHELL).ok().filter(|s| !s.is_empty()) {
        Some(v) => Ok(v),
        None => match env::var("SHELL").ok().filter(|s| !s.is_empty()) {
            Some(shell) => Ok(shell),
            None => {
                anyhow::bail!("No Q_SHELL or SHELL found");
            },
        },
    }
}

fn build_shell_command(command: Option<&[String]>) -> Result<CommandBuilder> {
    let mut builder = match command {
        Some(command) => {
            let mut iter = command.iter().map(|s| s.as_str());

            let mut builder = CommandBuilder::new(iter.next().unwrap());
            for arg in iter {
                builder.arg(arg);
            }
            builder
        },
        None => {
            let parent_shell = get_parent_shell()?;
            let mut builder = CommandBuilder::new(parent_shell);

            if env::var("Q_IS_LOGIN_SHELL").ok().as_deref() == Some("1") {
                builder.arg("--login");
            }

            if let Some(execution_string) = env::var("Q_EXECUTION_STRING").ok().filter(|s| !s.is_empty()) {
                builder.args(["-c", &execution_string]);
            }

            if let Some(extra_args) = env::var("Q_SHELL_EXTRA_ARGS").ok().filter(|s| !s.is_empty()) {
                builder.args(extra_args.split_whitespace().filter(|arg| arg != &"--login"));
            }

            builder
        },
    };

    builder.env(Q_TERM, env!("CARGO_PKG_VERSION"));
    if env::var_os("TMUX").is_some() {
        builder.env("Q_TERM_TMUX", env!("CARGO_PKG_VERSION"));
    }

    // Clean up environment and launch shell.
    builder.env_remove(Q_SHELL);
    builder.env_remove("Q_IS_LOGIN_SHELL");
    builder.env_remove("Q_START_TEXT");
    builder.env_remove("Q_SHELL_EXTRA_ARGS");
    builder.env_remove("Q_EXECUTION_STRING");

    if let Ok(dir) = std::env::current_dir() {
        builder.cwd(dir);
    }

    Ok(builder)
}

fn launch_shell(command: Option<&[String]>) -> Result<()> {
    let cmd = build_shell_command(command)?.as_command()?;
    let mut args: Vec<&OsStr> = std::vec![cmd.get_program()];
    args.extend(cmd.get_args());

    let cargs: Vec<_> = args
        .into_iter()
        .map(|arg| CString::new(arg.to_string_lossy().as_ref()).expect("Failed to convert arg to CString"))
        .collect();
    for (key, val) in cmd.get_envs() {
        // SAFETY: UNPROVEN — see docs/13-unsafe-review.md #24.
        // Operations: `std::env::set_var` / `std::env::remove_var`, unsafe since edition 2024.
        // Contract (std): "this function is safe to call in single-threaded programs... in
        // multi-threaded programs, the caller must ensure that no other thread reads or writes
        // the environment concurrently", because the implementation mutates the global
        // `environ` array without synchronisation and POSIX `getenv` may return a pointer into
        // it.
        // Evidence available: `launch_shell` runs on the main thread on the `figterm_main`
        // failure path, after `runtime.block_on` returned and the `Runtime` was dropped
        // (tokio's `Drop` shuts down and joins its worker threads).
        // Missing premise: two other threads can still be alive here, and neither is joined.
        // (a) `figterm_main`'s bare `std::thread::spawn(move || child_tx.send(child.wait()))`,
        // which is detached. (b) tracing-appender's writer thread, started by
        // `initialize_logging`. Neither touches the environment directly, but "no other thread
        // reads or writes the environment" quantifies over everything they transitively call,
        // including libc paths that read `environ`. Nothing in this frame establishes that.
        // Postcondition (intended): the child inherits the modified environment through the
        // `execvp` on the next lines, which is why this cannot simply be replaced by
        // `Command::envs`.
        unsafe {
            match val {
                Some(value) => env::set_var(key, value),
                None => {
                    env::remove_var(key);
                },
            }
        }
    }

    let err = execvp(&cargs[0], &cargs).unwrap_err();
    panic!("Failed to execvp: {err}");
}

fn figterm_main(command: Option<&[String]>) -> Result<()> {
    fig_settings::settings::init_global().ok();

    let session_id = match std::env::var("MOCK_QTERM_SESSION_ID") {
        Ok(id) => id,
        Err(_) => uuid::Uuid::new_v4().simple().to_string(),
    };

    // SAFETY:
    // Operation: `std::env::set_var`, unsafe since edition 2024.
    // Contract (std): safe in a single-threaded program; otherwise the caller must ensure no
    // other thread reads or writes the environment concurrently.
    // Evidence: the process is single-threaded here. Everything executed before this line is
    // `Cli::parse`, `logger::stdio_debug_log`, `uuid::Uuid::new_v4`, and the `fig_settings`
    // `state`/`settings` accessors. The last of those used to be the hole in this argument —
    // `fig_settings::sqlite` built an r2d2 pool whose `ScheduledThreadPool` started three worker
    // threads — but the pool is gone and a `Connection` starts none. `initialize_logging`
    // (tracing-appender's writer), the detached `child.wait()` thread, and the tokio runtime all
    // come later in this function.
    // Postcondition: `QTERM_SESSION_ID` is visible to every thread spawned afterwards and to the
    // shell spawned into the pty.
    unsafe {
        std::env::set_var(QTERM_SESSION_ID, &session_id);
    }

    let mut terminal = SystemTerminal::new_from_stdio()?;
    let screen_size = terminal.get_screen_size()?;

    let pty_size = PtySize {
        rows: screen_size.rows as u16,
        cols: screen_size.cols as u16,
        pixel_width: screen_size.xpixel as u16,
        pixel_height: screen_size.ypixel as u16,
    };

    let pty = open_pty(&pty_size).context("Failed to open pty")?;
    let command = build_shell_command(command)?;

    let pty_name = pty.slave.get_name().unwrap_or_else(|| session_id.clone());

    let _log_guard = match initialize_logging(LogArgs {
        log_level: None,
        log_to_stdout: false,
        log_file_path: Some(directories::logs_dir()?.join(format!("{PTY_BINARY_NAME}{pty_name}.log"))),
        delete_old_log_file: true,
    }) {
        Ok(logger_guard) => Some(logger_guard),
        Err(err) => {
            if !fig_settings::state::get_bool_or("pty.suppress_log_error", false) {
                // let id = capture_anyhow(&err);
                eprintln!("{PRODUCT_NAME} failed to init logger: {err:?}");
            }
            None
        },
    };

    logger::stdio_debug_log(format!("pty name: {pty_name}"));
    logger::stdio_debug_log("Forking child shell process");

    {
        let pid = nix::unistd::getpid();
        logger::stdio_debug_log(format!("Parent pid: {pid}"));
    }

    let mut child = pty.slave.spawn_command(command)?;
    info!("Shell: {:?}", child.process_id());
    if let Some(pid) = child.process_id() {
        logger::stdio_debug_log(format!("Child pid: {pid}"));
    }

    let (child_tx, mut child_rx) = oneshot::channel();
    std::thread::spawn(move || child_tx.send(child.wait()));

    info!("Pid: {}", std::process::id());
    info!("Pty name: {pty_name}");

    // Single-threaded on purpose: figterm is one `select!` loop plus a handful of I/O tasks, and
    // everything in that loop is awaited inline anyway, so worker-per-core bought nothing but
    // threads (one per core, per terminal tab). The one genuinely blocking consumer — the history
    // database — runs on the blocking pool instead; see `history::spawn_history_task`.
    let runtime = runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name_fn(|| {
            static ATOMIC_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            format!("{PTY_BINARY_NAME}-runtime-worker-{id}")
        })
        .build()?;

    let runtime_result = runtime.block_on(async {
        terminal.set_raw_mode()?;

        let (main_loop_tx, main_loop_rx) = main_loop_channel();

        let history_sender = history::spawn_history_task();

        // Spawn thread to handle figterm ipc
        let (figterm_socket_path, incoming_receiver) = spawn_figterm_ipc(&session_id).await?;

        // Spawn thread to handle remote ipc
        let (remote_sender, remote_receiver, stop_ipc_tx) = spawn_remote_ipc(
            session_id.clone(),
            main_loop_tx.clone()
        ).await?;

        let mut stdout = io::stdout();
        let mut master = pty.master.get_async_master_pty()?;

        let mut processor = Processor::new();
        let size = SizeInfo::new(pty_size.rows as usize, pty_size.cols as usize);
        let event_sender = EventHandler::new(remote_sender.clone(), history_sender.clone(), main_loop_tx.clone());
        let mut term = alacritty_terminal::Term::new(size, event_sender, 1, session_id.clone());

        let mut write_buffer: Vec<u8> = vec![0; BUFFER_SIZE];

        let mut key_interceptor = KeyInterceptor::new();
        key_interceptor.load_key_intercepts()?;

        // Only armed while an insertion lock is outstanding — see the `select!` arm below.
        let mut edit_buffer_interval = tokio::time::interval(Duration::from_millis(16));
        edit_buffer_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut first_time = true;

        let input_rx = terminal.read_input()?;

        let key_code_encode_mode = KeyCodeEncodeModes {
            encoding: KeyboardEncoding::Xterm,
            application_cursor_keys: false,
            newline_mode: false,
        };

        if let Ok(shell) = get_parent_shell() {
            let path = std::path::Path::new(&shell);
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or(shell.as_str());
            let title_osc = format!("\x1b]0;{name}\x07");
            if let Err(err) = stdout.write(title_osc.as_bytes()).await {
                error!("Failed to write title osc: {err}");
            }
        }

        // SIGTERM and SIGHUP are how this process is normally asked to go away: `kill`, a closing
        // terminal window, a logout. Their default disposition kills us outright, which skips
        // `UnixTerminal::drop` (the terminal stays in raw mode) and leaves the session socket
        // behind in the runtime dir. Handle them so the loop exits through the normal path.
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;

        let mut csi_u_set = false;

        let result: Result<()> = 'select_loop: loop {
            if first_time && term.shell_state().has_seen_prompt {
                trace!("Has seen prompt and first time");
                let initial_command = env::var("Q_START_TEXT").ok().filter(|s| !s.is_empty());
                if let Some(mut initial_command) = initial_command {
                    debug!("Sending initial text: {initial_command}");
                    initial_command.push('\n');
                    if let Err(err) = master.write_all(initial_command.as_bytes()).await {
                        error!("Failed to write initial command: {err}");
                    }
                }
                first_time = false;
            }

            let select_result: Result<()> = select! {
                biased;
                res = main_loop_rx.recv_async() => {
                    match res {
                        Ok(event) => {
                            match event {
                                MainLoopEvent::Insert { insert, unlock, bracketed, execute } => {
                                    use bstr::ByteSlice;
                                    if bracketed {
                                        if term.mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE) {
                                            master.write_all(b"\x1b[200~").await?;
                                            master.write_all(&insert.replace(b"\x1b", "")).await?;
                                            master.write_all(b"\x1b[201~").await?;
                                        } else {
                                            master.write_all(&insert.replace("\r\n", "\r").replace("\n", "\r")).await?;
                                        }
                                    } else {
                                        master.write_all(&insert).await?;
                                    }

                                    if execute {
                                        master.write_all(b"\r").await?; 
                                    }

                                    if unlock {
                                        key_interceptor.reset();
                                    }
                                },
                                MainLoopEvent::UnlockInterception => {
                                    key_interceptor.reset();
                                },
                                MainLoopEvent::SetImmediateMode(mode) => {
                                    if let Err(err) = terminal.set_immediate_mode(mode) {
                                        error!(%err, "Failed to set immediate mode");
                                    }
                                },
                                MainLoopEvent::SetCsiU => {
                                    // Send CSI > 1 u
                                    stdout.write_all(b"\x1b[>1u").await?;
                                    stdout.flush().await?;
                                    csi_u_set = true;
                                },
                                MainLoopEvent::UnsetCsiU => {
                                    // Send CSI < u
                                    stdout.write_all(b"\x1b[<u").await?;
                                    stdout.flush().await?;
                                    csi_u_set = false;
                                },
                            }
                        }
                        Err(err) => warn!("Failed to recv: {err}"),
                    };
                    Ok(())
                }
                res = input_rx.recv_async() => {
                    let mut input_res = Ok(());
                    match res {
                        Ok(events) => {
                            let mut write_buffer = BytesMut::new();
                            for event in events {
                                match event {
                                    Ok((raw, InputEvent::Key(event))) => {
                                        // Do not do most stuff during not preexec since that means a command is running
                                        let preexec = term.shell_state().preexec;

                                        debug!(?event, ?raw, %preexec,  "Got key event");

                                        // if we are in CSI u mode we try to encode first, otherwise we try to send the raw bytes first
                                        let raw = if csi_u_set {
                                            event.key.encode(event.modifiers, key_code_encode_mode, true)
                                                .ok()
                                                .map(|s| s.into_bytes().into()).or(raw)
                                        } else {
                                            raw.or_else(|| {
                                                event.key.encode(event.modifiers, key_code_encode_mode, true)
                                                    .ok()
                                                    .map(|s| s.into_bytes().into())
                                            })
                                        };

                                        let handled_action = if !preexec {
                                            if let Some(action) = key_interceptor.intercept_key(&event) {
                                                debug!(?action, "Intercepted action");
                                                let s = raw.clone()
                                                    .and_then(|b| String::from_utf8(b.to_vec()).ok())
                                                    .unwrap_or_default();
                                                let context = shell_state_to_context(term.shell_state());
                                                let hook = fig_proto::remote_hooks::new_intercepted_key_hook(context, action, s);
                                                if let Err(err) = remote_sender.send(hook_to_message(hook)) {
                                                    error!(%err, "Failed to send intercepted key hook");
                                                }

                                                if event.key == KeyCode::Escape {
                                                    key_interceptor.reset();
                                                }
                                                true
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        };

                                        if !handled_action
                                            && let Some(bytes) = raw {
                                                if (event.key == KeyCode::Char('c') || event.key == KeyCode::Char('d'))
                                                    && event.modifiers == Modifiers::CTRL {
                                                    key_interceptor.reset();
                                                }
                                                write_buffer.extend(&bytes);
                                            }
                                    }
                                    Ok((_, InputEvent::Resized)) => {
                                        terminal.flush()?;

                                        let size = terminal.get_screen_size()?;
                                        let pty_size = PtySize {
                                            rows: size.rows as u16,
                                            cols: size.cols as u16,
                                            pixel_width: size.xpixel as u16,
                                            pixel_height: size.ypixel as u16,
                                        };

                                        master.resize(pty_size)?;
                                        let window_size = SizeInfo::new(size.rows, size.cols);
                                        debug!("Window size changed: {window_size:?}");
                                        term.resize(window_size);
                                    }
                                    Ok((None, InputEvent::Paste(string))) => {
                                        // Pass through bracketed pastes.
                                        if term.mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE) {
                                            write_buffer.extend(b"\x1b[200~");
                                            write_buffer.extend(string.replace('\x1b', "").as_bytes());
                                            write_buffer.extend(b"\x1b[201~");
                                        } else {
                                            write_buffer.extend(string.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
                                        }
                                    }
                                    Ok((raw, _)) => {
                                        if let Some(raw) = raw {
                                            info!("Fallback write");
                                            write_buffer.extend(&raw);
                                        } else {
                                            info!("Unhandled input event with no raw pass-through data");
                                        }
                                    }
                                    Err(err) => {
                                        error!("Failed receiving input from stdin: {err}");
                                        input_res = Err(err);
                                        break;
                                    }
                                };
                            }
                            master.write_all(&write_buffer).await?;
                        }
                        Err(err) => {
                            warn!("Failed recv: {err}");
                        }
                    };
                    input_res
                }
                res = master.read(&mut write_buffer) => {
                    #[cfg(feature = "profiling_early_exit")]
                    break 'select_loop Ok(());
                    match res {
                        Ok(0) => {
                            trace!("EOF from master");
                            break 'select_loop Ok(());
                        },
                        Ok(size) => {
                            trace!("Read {size} bytes from master");

                            let old_delayed_count = term.get_delayed_events_count();
                            processor.advance(&mut term, &write_buffer[..size]);

                            let delayed_count = term.get_delayed_events_count();

                            // We have delayed events and did not receive delayed events. Flush all
                            // delayed events now.
                            if delayed_count > 0 && delayed_count == old_delayed_count {
                                term.flush_delayed_events();
                            }

                            stdout.write_all(&write_buffer[..size]).await?;
                            stdout.flush().await?;

                            if let Some(edit_buffer) = can_send_edit_buffer(&term)
                                && let Err(err) = send_edit_buffer(term.shell_state(), edit_buffer, &remote_sender, None).await {
                                    warn!("Failed to send edit buffer: {err}");
                                }

                            Ok(())
                        }
                        Err(err) => {
                            error!("Failed to read from master: {err}");
                            break 'select_loop Ok(());
                        }
                    }
                }
                msg = remote_receiver.recv_async() => {
                    match msg {
                        Ok(message) => {
                            trace!("Received message from socket: {message:?}");
                            process_remote_message(
                                message,
                                remote_sender.clone(),
                                &term,
                                &mut master,
                                &mut key_interceptor
                            ).await?;
                        }
                        Err(err) => {
                            error!("Failed to receive message from socket: {err}");
                        }
                    }
                    Ok(())
                }
                msg = incoming_receiver.recv_async() => {
                    match msg {
                        Ok((message, sender)) => {
                            debug!("Received message from figterm listener: {message:?}");
                            process_figterm_message(
                                message,
                                sender.clone(),
                                &term,
                                &mut master,
                                &mut key_interceptor,
                            ).await?;
                        }
                        Err(err) => {
                            error!("Failed to receive message from socket: {err}");
                        }
                    }
                    Ok(())
                }
                // Re-send the edit buffer once an insertion lock expires. The guard is what keeps
                // this cheap: a disabled `select!` branch is never polled, so no timer is
                // registered and the task is not woken at all. Unguarded, this interval woke every
                // figterm on the machine 62.5 times a second forever, and the body then took a
                // lock and immediately discarded the tick.
                _ = edit_buffer_interval.tick(), if INSERTION_LOCKED_AT.read_ignore_poison().is_some() => {
                    if let Some(edit_buffer) = can_send_edit_buffer(&term)
                        && let Err(err) = send_edit_buffer(term.shell_state(), edit_buffer, &remote_sender, None).await {
                            warn!(%err, "Failed to send edit buffer");
                        }
                    Ok(())
                }
                _ = &mut child_rx => {
                    trace!("Shell process exited");
                    break 'select_loop Ok(());
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, shutting down");
                    break 'select_loop Ok(());
                }
                _ = sighup.recv() => {
                    info!("Received SIGHUP, shutting down");
                    break 'select_loop Ok(());
                }
            };

            if let Err(err) = select_result {
                error!("Error in select loop: {err}");
                break 'select_loop Err(err);
            }
        };

        let _ = stop_ipc_tx.send(());

        // Nothing else unlinks this: `spawn_figterm_ipc` only clears a *stale* socket when it
        // binds, so without this every session that ended left a dead socket in the runtime dir.
        if let Err(err) = tokio::fs::remove_file(&figterm_socket_path).await {
            debug!(%err, path = ?figterm_socket_path, "Failed to remove the session socket");
        }

        result
    });

    // Reading from stdin is a blocking task on a separate thread:
    // https://github.com/tokio-rs/tokio/issues/2466
    // We must explicitly shutdown the runtime to exit.
    // This can cause resource leaks if we aren't careful about tasks we spawn.
    runtime.shutdown_background();

    runtime_result
}

fn main() {
    let cli = Cli::parse();
    let command = cli.command.as_deref();

    logger::stdio_debug_log(format!("{Q_LOG_LEVEL}={}", fig_log::get_log_level()));

    if !state::get_bool_or("figterm.enabled", true) {
        println!("[NOTE] {PTY_BINARY_NAME} is disabled. Autocomplete will not work.");
        logger::stdio_debug_log("figterm is disabled. `figterm.enabled` == false");
        return;
    }

    match figterm_main(command) {
        Ok(()) => {
            info!("Exiting");
        },
        Err(err) => {
            error!("Error in async runtime: {err}");
            println!("{PRODUCT_NAME} had an Error!: {err:?}");
            // capture_anyhow(&err);

            // Fallback to normal shell
            if let Err(err) = launch_shell(command) {
                // capture_anyhow(&err);
                logger::stdio_debug_log(err.to_string());
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The main-loop channel is drained by the same `select!` loop that fills it, so a bounded
    /// channel deadlocks figterm against itself on a burst deeper than the bound.
    #[test]
    fn main_loop_channel_is_unbounded() {
        let (tx, rx) = main_loop_channel();
        assert_eq!(tx.capacity(), None, "main loop channel must not be bounded");

        // With `bounded(16)` this loop parks forever: nothing drains `rx`.
        for i in 0..10_000 {
            tx.send(MainLoopEvent::SetImmediateMode(i % 2 == 0))
                .expect("send to a live receiver must not fail");
        }
        assert_eq!(rx.len(), 10_000);
    }

    /// `can_send_edit_buffer` runs after every read from the pty, so a command printing in the
    /// background used to re-send a byte-identical edit buffer for each chunk of its output.
    #[tokio::test]
    async fn identical_edit_buffers_are_sent_once() {
        let (tx, rx) = flume::unbounded();
        let shell_state = ShellState::default();
        let buffer = |text: &str, cursor: usize| TextBuffer {
            buffer: text.into(),
            cursor_idx: Some(cursor),
        };

        crate::ipc::set_desktop_connected(true);
        forget_last_sent_edit_buffer();

        send_edit_buffer(&shell_state, buffer("git st", 6), &tx, None)
            .await
            .unwrap();
        assert_eq!(rx.len(), 1, "the first hook must always go out");

        for _ in 0..100 {
            send_edit_buffer(&shell_state, buffer("git st", 6), &tx, None)
                .await
                .unwrap();
        }
        assert_eq!(rx.len(), 1, "identical buffers must be suppressed");

        // A moved cursor is a different edit buffer even with the same text.
        send_edit_buffer(&shell_state, buffer("git st", 3), &tx, None)
            .await
            .unwrap();
        assert_eq!(rx.len(), 2);

        send_edit_buffer(&shell_state, buffer("git sta", 7), &tx, None)
            .await
            .unwrap();
        assert_eq!(rx.len(), 3);

        // A reconnect must not suppress the first hook of the new session.
        forget_last_sent_edit_buffer();
        send_edit_buffer(&shell_state, buffer("git sta", 7), &tx, None)
            .await
            .unwrap();
        assert_eq!(rx.len(), 4);

        crate::ipc::set_desktop_connected(false);
    }

    #[test]
    fn autocomplete_enabled_test() {
        assert!(autocomplete_enabled(&Env::new_fake()));
        assert!(autocomplete_enabled(&Env::from_slice(&[(Q_DISABLE_AUTOCOMPLETE, "")])));
        assert!(!autocomplete_enabled(&Env::from_slice(&[(
            Q_DISABLE_AUTOCOMPLETE,
            "1"
        )])));
        assert!(!autocomplete_enabled(&Env::from_slice(&[(
            Q_DISABLE_AUTOCOMPLETE,
            "1"
        )])));
    }
}
