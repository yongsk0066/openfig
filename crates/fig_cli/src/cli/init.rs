use std::borrow::Cow;
use std::fmt::Display;
use std::io::{
    Write,
    stdout,
};
use std::path::Path;
use std::process::ExitCode;
use std::sync::LazyLock;

use clap::Args;
use crossterm::style::Stylize;
use eyre::Result;
use fig_integrations::shell::{
    ShellExt,
    When,
    ZSH_HOOK_SETTING,
    zsh_hook_source,
};
use fig_os_shim::Context;
use fig_util::env_var::FIG_SHELL;
use fig_util::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
    Shell,
    get_parent_process_exe,
};

use super::internal::should_figterm_launch::should_figterm_launch_exit_status;

const SHELL_INTEGRATIONS_ENABLED_STATE_KEY: &str = "shell-integrations.enabled";

static IS_SNAPSHOT_TEST: LazyLock<bool> = LazyLock::new(|| fig_os_shim::Env::new().fig_init_snapshot_test());

#[derive(Debug, Args, PartialEq, Eq)]
pub struct InitArgs {
    /// The shell to generate the dotfiles for
    #[arg(value_enum)]
    shell: Shell,
    /// When to generate the dotfiles for
    #[arg(value_enum)]
    when: When,
    #[arg(long)]
    rcfile: Option<String>,
}

impl InitArgs {
    pub async fn execute(&self) -> Result<ExitCode> {
        let InitArgs { shell, when, rcfile } = self;
        match shell_init(shell, when, rcfile).await {
            Ok(source) => writeln!(stdout(), "{source}"),
            Err(err) => writeln!(stdout(), "# Could not load source: {err}"),
        }
        .ok();
        Ok(ExitCode::SUCCESS)
    }
}

#[derive(PartialEq, Eq)]
enum GuardAssignment {
    AfterSourcing,
}

fn assign_shell_variable(shell: &Shell, name: impl Display, value: impl Display, exported: bool) -> String {
    match (shell, exported) {
        (Shell::Bash | Shell::Zsh, false) => format!("{name}=\"{value}\""),
        (Shell::Bash | Shell::Zsh, true) => format!("export {name}=\"{value}\""),
    }
}

fn guard_source(
    shell: &Shell,
    export: bool,
    guard_var: impl Display,
    assignment: GuardAssignment,
    source: impl Into<Cow<'static, str>>,
) -> String {
    let mut output: Vec<Cow<'static, str>> = Vec::with_capacity(4);
    output.push(match shell {
        Shell::Bash | Shell::Zsh => format!("if [ -z \"${{{guard_var}}}\" ]; then").into(),
    });

    let shell_var = assign_shell_variable(shell, guard_var, "1", export);
    match assignment {
        GuardAssignment::AfterSourcing => {
            for line in source.into().lines() {
                output.push(format!("  {line}").into());
            }
            output.push(format!("  {shell_var}").into());
        },
    }

    output.push(
        match shell {
            Shell::Bash | Shell::Zsh => "fi\n",
        }
        .into(),
    );

    output.join("\n")
}

async fn shell_init(shell: &Shell, when: &When, rcfile: &Option<String>) -> Result<String> {
    // Do not print any shell integrations for `.profile` as it can cause issues on launch
    if std::env::consts::OS == "linux" && matches!(rcfile.as_deref(), Some("profile")) {
        return Ok("".into());
    }

    if !fig_settings::state::get_bool_or(SHELL_INTEGRATIONS_ENABLED_STATE_KEY, true) {
        return Ok(shell_integrations_disabled_code(*shell));
    }

    // Snapshots must not depend on the settings of whoever runs them.
    let zsh_hook =
        *shell == Shell::Zsh && !*IS_SNAPSHOT_TEST && fig_settings::settings::get_bool_or(ZSH_HOOK_SETTING, false);

    let mut to_source = Vec::new();

    if let Some(parent_process) = get_parent_process_exe() {
        to_source.push(assign_shell_variable(
            shell,
            FIG_SHELL,
            if *IS_SNAPSHOT_TEST {
                Path::new("/bin/zsh").display()
            } else {
                parent_process.display()
            },
            false,
        ));
    };

    if when == &When::Pre {
        let status = if *IS_SNAPSHOT_TEST {
            0
        } else if zsh_hook {
            // `pre.sh` execs the pty binary only for 0, or 2 with no `FIG_TERM`.
            1
        } else {
            should_figterm_launch_exit_status(&Context::new(), true)
        };
        to_source.push(assign_shell_variable(shell, "SHOULD_FIGTERM_LAUNCH", status, false));
    }

    if let Some(path) = fig_settings::settings::get_string_opt("figterm.path") {
        to_source.push(assign_shell_variable(shell, "FIG_TERM_PATH", path, false));
    }

    to_source.push(integration_source(shell, when, zsh_hook)?);

    Ok(to_source.join("\n"))
}

/// The `pre`/`post` script for `shell`. With the zsh hook on, `post` is `fig.zsh` — unless the
/// shell already runs under the pty binary (`FIG_TERM`: a tab opened before the setting changed,
/// or `FIG_FORCE_FIGTERM_LAUNCH`), which only understands the OSC markers of `post.zsh`. Reporting
/// both ways at once would give the desktop two sessions for one prompt.
fn integration_source(shell: &Shell, when: &When, zsh_hook: bool) -> Result<String> {
    let script = shell.get_fig_integration_source(when);
    if !(zsh_hook && *shell == Shell::Zsh && *when == When::Post) {
        return Ok(script);
    }
    Ok(format!(
        "if [[ -n \"${{FIG_TERM:-}}\" ]]; then\n{script}\nelse\n{hook}\nfi\n",
        hook = zsh_hook_source()?
    ))
}

fn shell_integrations_disabled_code(shell: Shell) -> String {
    guard_source(
        &shell,
        false,
        "FIG_SHELL_INTEGRATION_DISABLED",
        GuardAssignment::AfterSourcing,
        format!(
            "printf '{PRODUCT_NAME} shell integration is disabled.\\nRe-enable by running: {}\\n'",
            format!("{CLI_BINARY_NAME} _ local-state -d {SHELL_INTEGRATIONS_ENABLED_STATE_KEY}").magenta()
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::process::{
        Command,
        Stdio,
    };

    use super::*;

    fn run_shell_stdout(shell: &Shell, text: &str) -> String {
        let mut child = Command::new(shell.as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let stdin = child.stdin.as_mut().unwrap();
        // Since these are all guarded we run the code twice to double check
        stdin.write_all(text.as_bytes()).unwrap();
        stdin.write_all(text.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let output = child.wait_with_output().unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    /// With the hook on, zsh's `pre` never starts the pty binary and `post` carries `fig.zsh` for a
    /// shell outside the pty binary and `post.zsh` for one inside it.
    #[test]
    fn zsh_hook_replaces_the_markers_outside_the_pty_binary() {
        let post = integration_source(&Shell::Zsh, &When::Post, true).unwrap();
        assert!(post.contains("zsocket"), "fig.zsh missing");
        assert!(post.contains("OSCUnlock"), "post.zsh missing for the pty-binary branch");
        assert!(!post.contains("{{"), "unfilled placeholder in {post}");

        let off = integration_source(&Shell::Zsh, &When::Post, false).unwrap();
        assert!(!off.contains("zsocket"));
        assert_eq!(
            integration_source(&Shell::Zsh, &When::Pre, true).unwrap(),
            Shell::Zsh.get_fig_integration_source(&When::Pre)
        );
        assert!(
            !integration_source(&Shell::Bash, &When::Post, true)
                .unwrap()
                .contains("zsocket")
        );

        // Inside the pty binary the markers still go out and the hook is never defined.
        let run = |fig_term: &str| {
            let output = Command::new("/bin/zsh")
                .args([
                    "-f",
                    "-c",
                    &format!(
                        "{post}\nprint -r -- \"hook=${{+functions[_fig_report]}} markers=${{+functions[fig_precmd]}}\""
                    ),
                ])
                .env("FIG_TERM", fig_term)
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert!(run("1").ends_with("hook=0 markers=1\n"), "{}", run("1"));
        // Not interactive, so `fig.zsh` defines nothing either; the interactive half is figterm2's.
        assert!(run("").ends_with("hook=0 markers=0\n"), "{}", run(""));
    }

    #[test]
    fn test_prompts() {
        for shell in Shell::all_test() {
            let shell_integrations_disabled_output = run_shell_stdout(&shell, &shell_integrations_disabled_code(shell));

            println!("=== shell_integrations_disabled_code {shell:?} ===");
            println!("{shell_integrations_disabled_output}");
            println!("===");

            assert_eq!(
                shell_integrations_disabled_output,
                format!(
                    "{PRODUCT_NAME} shell integration is disabled.\nRe-enable by running: {}\n",
                    format!("{CLI_BINARY_NAME} _ local-state -d {SHELL_INTEGRATIONS_ENABLED_STATE_KEY}").magenta()
                )
            );
        }
    }
}
