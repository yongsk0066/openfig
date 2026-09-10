use alacritty_terminal::event::{
    Event,
    EventListener,
};
use alacritty_terminal::term::ShellState;
use fig_proto::remote::Hostbound;
use fig_proto::remote_hooks::{
    hook_to_message,
    new_postexec_hook,
    new_preexec_hook,
    new_prompt_hook,
};
use flume::Sender;
use tracing::level_filters::LevelFilter;
use tracing::{
    debug,
    error,
};

use crate::history::{
    HistoryCommand,
    HistorySender,
};
use crate::ipc::desktop_connected;
use crate::{
    MainLoopEvent,
    shell_state_to_context,
};

pub struct EventHandler {
    socket_sender: Sender<Hostbound>,
    history_sender: HistorySender,
    main_loop_sender: Sender<MainLoopEvent>,
    csi_u_enabled: bool,
}

impl EventHandler {
    pub fn new(
        socket_sender: Sender<Hostbound>,
        history_sender: HistorySender,
        main_loop_sender: Sender<MainLoopEvent>,
    ) -> Self {
        Self {
            socket_sender,
            history_sender,
            main_loop_sender,
            csi_u_enabled: fig_settings::settings::get_bool_or("figterm.csi-u.enabled", false),
        }
    }
}

impl EventListener for EventHandler {
    fn send_event(&self, event: Event<'_>, shell_state: &ShellState) {
        debug!(?event, ?shell_state, "Handling event");
        match event {
            Event::Prompt => {
                if let Some(cwd) = &shell_state.local_context.current_working_directory
                    && cwd.exists()
                {
                    std::env::set_current_dir(cwd).ok();
                }

                if let Err(err) = self.main_loop_sender.send(MainLoopEvent::SetImmediateMode(false)) {
                    error!(%err, "Sender error");
                }

                if desktop_connected() {
                    let hook = new_prompt_hook(Some(shell_state_to_context(shell_state)));
                    if let Err(err) = self.socket_sender.send(hook_to_message(hook)) {
                        error!(%err, "Sender error");
                    }
                }

                if self.csi_u_enabled
                    && let Err(err) = self.main_loop_sender.send(MainLoopEvent::SetCsiU)
                {
                    error!(%err, "Sender error");
                }
            },
            Event::PreExec => {
                if let Err(err) = self.main_loop_sender.send(MainLoopEvent::UnlockInterception) {
                    error!(%err, "Sender error");
                }

                if let Err(err) = self.main_loop_sender.send(MainLoopEvent::SetImmediateMode(true)) {
                    error!(%err, "Sender error");
                }

                if desktop_connected() {
                    let hook = new_preexec_hook(Some(shell_state_to_context(shell_state)));
                    if let Err(err) = self.socket_sender.send(hook_to_message(hook)) {
                        error!(%err, "Sender error");
                    }
                }

                if self.csi_u_enabled
                    && let Err(err) = self.main_loop_sender.send(MainLoopEvent::UnsetCsiU)
                {
                    error!(%err, "Sender error");
                }
            },
            Event::CommandInfo(command_info) => {
                if desktop_connected() {
                    let context = shell_state_to_context(shell_state);
                    let hook = new_postexec_hook(context, command_info.command.clone(), command_info.exit_code);
                    if let Err(err) = self.socket_sender.send(hook_to_message(hook)) {
                        error!(%err, "Sender error");
                    }
                }

                // History is local state, not an IPC hook: record it whether or not the desktop
                // app is attached.
                if let Err(err) = self.history_sender.send(HistoryCommand::Insert(command_info.clone())) {
                    error!(%err, "Sender error");
                }
            },
            Event::ShellChanged => {
                // let shell = &shell_state.local_context.shell;
                // configure_scope(|scope| {
                //     if let Some(shell) = shell {
                //         scope.set_tag("shell", shell);
                //     }
                // });
            },
        }
    }

    fn log_level_event(&self, level: Option<String>) {
        if let Err(err) = fig_log::set_log_level(level.unwrap_or_else(|| LevelFilter::INFO.to_string())) {
            error!(%err, "Failed to set log level");
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::CommandInfo;

    use super::*;
    use crate::ipc::set_desktop_connected;

    /// The receivers are returned so the caller keeps them alive; a dropped receiver would make
    /// the sends fail for the wrong reason.
    #[allow(clippy::type_complexity)]
    fn handler() -> (
        EventHandler,
        flume::Receiver<Hostbound>,
        flume::Receiver<HistoryCommand>,
        flume::Receiver<MainLoopEvent>,
    ) {
        let (socket_sender, socket_rx) = flume::unbounded();
        let (history_sender, history_rx) = flume::unbounded();
        let (main_loop_sender, main_loop_rx) = flume::unbounded();
        (
            EventHandler {
                socket_sender,
                history_sender,
                main_loop_sender,
                csi_u_enabled: false,
            },
            socket_rx,
            history_rx,
            main_loop_rx,
        )
    }

    /// While the desktop app is away nothing drains the outgoing channel and everything queued is
    /// dropped by `outgoing_rx.drain()` at the next handshake, so hooks must not be queued at all.
    #[test]
    fn hooks_are_not_queued_while_disconnected() {
        let (handler, socket_rx, history_rx, _main_loop_rx) = handler();
        let shell_state = ShellState::default();
        let command_info = CommandInfo {
            command: Some("echo hi".into()),
            shell: Some("zsh".into()),
            pid: None,
            session_id: None,
            cwd: None,
            start_time: None,
            end_time: None,
            username: None,
            exit_code: Some(0),
        };

        set_desktop_connected(false);
        for _ in 0..1_000 {
            handler.send_event(Event::Prompt, &shell_state);
            handler.send_event(Event::PreExec, &shell_state);
            handler.send_event(Event::CommandInfo(&command_info), &shell_state);
        }
        assert_eq!(socket_rx.len(), 0, "hooks were queued with no desktop app attached");
        // Command history is local state and must still be recorded.
        assert_eq!(history_rx.len(), 1_000);

        // The very first hook after a reconnect must go out.
        set_desktop_connected(true);
        handler.send_event(Event::Prompt, &shell_state);
        assert_eq!(socket_rx.len(), 1);
        handler.send_event(Event::PreExec, &shell_state);
        handler.send_event(Event::CommandInfo(&command_info), &shell_state);
        assert_eq!(socket_rx.len(), 3);

        set_desktop_connected(false);
    }
}
