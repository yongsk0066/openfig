pub mod golden;
pub mod hook;
pub mod latency;

use anyhow::{
    Context as _,
    Result,
};

/// Path to the workspace build of the CLI (`target/<profile>/fig_cli`).
///
/// The test binary lives in `target/<profile>/deps/`, so the sibling of its grandparent directory
/// is the profile directory. `cargo nextest run` builds the workspace's bin targets, so this is
/// always present when the tests are run through the normal gate. The shells here load this
/// build's integration through it, never the installed CLI's.
pub fn cli_binary() -> Result<std::path::PathBuf> {
    let test_exe = std::env::current_exe().context("no current_exe")?;
    let profile_dir = test_exe
        .parent()
        .and_then(|deps| deps.parent())
        .context("test binary is not under target/<profile>/deps")?;
    let path = profile_dir.join(fig_util::consts::CLI_CRATE_NAME);
    if !path.exists() {
        anyhow::bail!(
            "{} was not built at {}; run the tests through `cargo nextest run --workspace`",
            fig_util::consts::CLI_CRATE_NAME,
            path.display()
        );
    }
    Ok(path)
}
