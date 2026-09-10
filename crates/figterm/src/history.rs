use flume::Sender;
use tracing::{
    error,
    trace,
};

use crate::HOSTNAME;

pub enum HistoryCommand {
    Insert(alacritty_terminal::term::CommandInfo),
}

pub type HistorySender = Sender<HistoryCommand>;

pub async fn spawn_history_task() -> HistorySender {
    trace!("Spawning history task");

    let (sender, receiver) = flume::bounded::<HistoryCommand>(64);

    tokio::task::spawn(async move {
        let history = fig_settings::history::History::new();

        while let Ok(command) = receiver.recv_async().await {
            match command {
                HistoryCommand::Insert(command) => {
                    let command_info = fig_settings::history::CommandInfo {
                        command: command.command,
                        shell: command.shell,
                        pid: command.pid,
                        session_id: command.session_id,
                        cwd: command.cwd,
                        start_time: command.start_time,
                        end_time: command.end_time,
                        hostname: command
                            .username
                            .as_deref()
                            .and_then(|username| HOSTNAME.as_deref().map(|hostname| format!("{username}@{hostname}"))),
                        exit_code: command.exit_code,
                    };

                    if let Err(err) = history.insert_command_history(&command_info) {
                        error!(%err, "Failed to insert command into history");
                    }
                },
            }
        }
    });

    sender
}
