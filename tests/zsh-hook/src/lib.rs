pub mod golden;
pub mod hook;
pub mod latency;

use anyhow::{
    Context as _,
    Result,
};

/// Path to this workspace build's CLI, never the installed application.
///
/// Prefer the runner's explicit path. Cargo 1.100 puts test executables under
/// `<profile>/build/<package>/<hash>/out/`; older Cargo used `<profile>/deps/`.
fn profile_dir(executable: &std::path::Path) -> Option<&std::path::Path> {
    executable.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?;
        (name == "deps" || name == "build").then(|| ancestor.parent()).flatten()
    })
}

pub fn cli_binary() -> Result<std::path::PathBuf> {
    let name = fig_util::consts::CLI_CRATE_NAME;
    let path = if let Some(path) = std::env::var_os(format!("NEXTEST_BIN_EXE_{name}"))
        .or_else(|| std::env::var_os(format!("CARGO_BIN_EXE_{name}")))
    {
        path.into()
    } else {
        let test_exe = std::env::current_exe().context("no current_exe")?;
        profile_dir(&test_exe)
            .context("unrecognized Cargo test executable layout")?
            .join(name)
    };
    if !path.is_file() {
        anyhow::bail!(
            "{name} was not built at {}; run the tests through `cargo nextest run --workspace`",
            path.display()
        );
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::profile_dir;

    #[test]
    fn resolves_old_and_new_cargo_layouts_including_cross_compilation() {
        for (exe, profile) in [
            ("/repo/target/debug/deps/hook-123", "/repo/target/debug"),
            (
                "/repo/target/debug/build/zsh-hook-tests/123/out/hook-123",
                "/repo/target/debug",
            ),
            (
                "/custom/aarch64-apple-darwin/release/build/zsh-hook-tests/123/out/hook-123",
                "/custom/aarch64-apple-darwin/release",
            ),
        ] {
            assert_eq!(profile_dir(Path::new(exe)), Some(Path::new(profile)));
        }
        assert_eq!(profile_dir(Path::new("/Applications/Fig.app/Contents/MacOS/q")), None);
    }
}
