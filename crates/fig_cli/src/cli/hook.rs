use std::process::ExitCode;

use clap::Subcommand;
use eyre::Result;
use fig_ipc::local::send_hook_to_socket;
use fig_proto::hooks;

#[derive(Debug, PartialEq, Eq, Subcommand)]
#[command(hide = true)]
pub enum HookSubcommand {
    ClearAutocompleteCache {
        #[arg(long)]
        cli: Vec<String>,
    },
}

impl HookSubcommand {
    pub async fn execute(&self) -> Result<ExitCode> {
        // Hooks should exit silently on failure.
        match self.execute_hook().await {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(_) => Ok(ExitCode::FAILURE),
        }
    }

    pub async fn execute_hook(&self) -> Result<()> {
        let hook = match self {
            HookSubcommand::ClearAutocompleteCache { cli } => hooks::new_clear_autocomplete_cache(cli.clone()),
        };

        Ok(send_hook_to_socket(hook).await?)
    }
}
