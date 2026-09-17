use std::borrow::Cow;
use std::io::{
    Write,
    stdout,
};
use std::process::ExitCode;

use fig_integrations::shell::ZSH_HOOK_SETTING;
use fig_os_shim::Context;
use fig_util::{
    PTY_BINARY_NAME,
    Shell,
};

const FIG_FORCE_FIGTERM_LAUNCH: &str = "FIG_FORCE_FIGTERM_LAUNCH";
const FIG_TERM_DISABLED: &str = "FIG_TERM_DISABLED";
const INSIDE_EMACS: &str = "INSIDE_EMACS";
const TERM_PROGRAM: &str = "TERM_PROGRAM";
const WARP_TERMINAL: &str = "WarpTerminal";

#[allow(dead_code)]
struct ProcessInfo {
    pid: Box<fig_os_shim::process_info::Pid>,
    exe_name: String,
    cmdline_name: Option<String>,
    is_valid: bool,
    is_special: bool,
}

enum Status {
    Launch(Cow<'static, str>),
    DontLaunch(Cow<'static, str>),
    Process(ProcessInfo),
}

impl Status {
    fn exit_status(self, quiet: bool) -> Result<ProcessInfo, u8> {
        match self {
            Status::Process(info) => Ok(info),
            Status::Launch(s) => {
                if !quiet {
                    writeln!(stdout(), "✅ {s}").ok();
                }
                Err(0)
            },
            Status::DontLaunch(s) => {
                if !quiet {
                    writeln!(stdout(), "❌ {s}").ok();
                }
                Err(1)
            },
        }
    }
}

fn parent_status(ctx: &Context, current_pid: fig_os_shim::process_info::Pid, zsh_hook: bool) -> Status {
    use fig_util::env_var::FIG_TERM;
    let env = ctx.env();

    let parent_pid = match current_pid.parent() {
        Some(pid) => pid,
        None => return Status::DontLaunch("No parent PID".into()),
    };

    let parent_path = match parent_pid.exe() {
        Some(path) => path,
        None => return Status::DontLaunch("No parent path".into()),
    };

    let parent_name = match parent_path.file_name() {
        Some(name) => match name.to_str() {
            Some(name) => name,
            None => return Status::DontLaunch("Parent name is not valid unicode".into()),
        },
        None => return Status::DontLaunch("No parent name".into()),
    };

    // A shell whose parent *is* figterm (`zsh (figterm)` is the per-shell copy of the pty
    // binary) is already wrapped; launching again would nest figterm inside figterm forever.
    // The environment normally says so too (`FIG_TERM`), but a stale copy speaking older
    // variable names must not be able to defeat this check.
    if parent_name == PTY_BINARY_NAME || parent_name.ends_with(&format!("({PTY_BINARY_NAME})")) {
        return Status::DontLaunch(format!("Parent {parent_name:?} is {PTY_BINARY_NAME}").into());
    }

    // `Shell::try_find_shell` is the same test `Shell::current_shell` uses, so this cannot drift
    // from the set of shells the integration actually supports.
    let parent_shell = Shell::try_find_shell(parent_name);
    let valid_parent = parent_shell.is_some();

    // With the hook on, zsh reports its own edit buffer (`fig.zsh`); wrapping it as well would
    // report every prompt twice.
    if zsh_hook && parent_shell == Some(Shell::Zsh) {
        return Status::DontLaunch(format!("zsh uses the shell hook ({ZSH_HOOK_SETTING})").into());
    }

    if env.in_codespaces() {
        return match env.get_os(FIG_TERM) {
            Some(_) => Status::DontLaunch(format!("In Codespaces and {FIG_TERM} is set").into()),
            None => Status::Launch(format!("In Codespaces and {FIG_TERM} is not set").into()),
        };
    }

    Status::Process(ProcessInfo {
        pid: parent_pid,
        exe_name: parent_name.into(),
        cmdline_name: None,
        is_valid: valid_parent,
        is_special: false,
    })
}

fn grandparent_status(parent_pid: fig_os_shim::process_info::Pid) -> Status {
    let Some(grandparent_pid) = parent_pid.parent() else {
        return Status::DontLaunch("No grandparent PID".into());
    };

    let Some(grandparent_path) = grandparent_pid.exe() else {
        return Status::DontLaunch("No grandparent path".into());
    };

    let grandparent_name = match grandparent_path.file_name() {
        Some(name) => match name.to_str() {
            Some(name) => name,
            None => return Status::DontLaunch("Grandparent name is not a valid utf8 str".into()),
        },
        None => return Status::DontLaunch("No grandparent name".into()),
    };

    // The real nesting shape is figterm -> shell -> `q _ should-figterm-launch`: the shell is
    // the parent, figterm is the *grandparent*. Refuse here, whatever the environment says
    // (a stale copy speaking older variable names started figterm inside figterm on 2026-09-11).
    if grandparent_name == PTY_BINARY_NAME || grandparent_name.ends_with(&format!("({PTY_BINARY_NAME})")) {
        return Status::DontLaunch(format!("Grandparent {grandparent_name:?} is {PTY_BINARY_NAME}").into());
    }

    let grandparent_cmdline_name = if let Some(cmdline) = grandparent_pid.cmdline() {
        cmdline
            .split(' ')
            .take(1)
            .next()
            .and_then(|cmd| cmd.split('/').next_back())
            .map(str::to_string)
    } else {
        None
    };

    // The terminals the platform supports
    let terminals = fig_util::terminal::MACOS_TERMINALS
        .iter()
        .chain(fig_util::terminal::SPECIAL_TERMINALS)
        .cloned()
        .collect::<Vec<_>>();

    // Try to find if any of the supported terminals matches the current grandparent process
    let valid_grandparent = terminals
        .iter()
        .find(|terminal| terminal.executable_names().contains(&grandparent_name));

    Status::Process(ProcessInfo {
        pid: grandparent_pid,
        exe_name: grandparent_name.into(),
        cmdline_name: grandparent_cmdline_name,
        is_valid: valid_grandparent.is_some(),
        is_special: valid_grandparent.is_some_and(|term| term.is_special()),
    })
}

fn should_launch(ctx: &Context, quiet: bool, zsh_hook: bool) -> u8 {
    let process_info = ctx.process_info();
    let current_pid = process_info.current_pid();

    let parent_info = match parent_status(ctx, current_pid, zsh_hook).exit_status(quiet) {
        Ok(info) => info,
        Err(i) => return i,
    };
    let grandparent_info = match grandparent_status(*parent_info.pid.clone()).exit_status(quiet) {
        Ok(info) => info,
        Err(i) => return i,
    };

    if !quiet {
        let ancestry = format!(
            "{} {} | {} ({}) <- {} {} ({})",
            if grandparent_info.is_valid { "✅" } else { "❌" },
            grandparent_info.exe_name,
            grandparent_info.cmdline_name.unwrap_or_default(),
            grandparent_info.pid.clone(),
            if parent_info.is_valid { "✅" } else { "❌" },
            parent_info.exe_name,
            parent_info.pid.clone(),
        );

        writeln!(stdout(), "{ancestry}").ok();
    }

    if !grandparent_info.is_special {
        if !quiet {
            writeln!(stdout(), "🟡 Falling back to old mechanism since on macOS").ok();
        }
        return 2;
    }

    u8::from(!(grandparent_info.is_valid && parent_info.is_valid))
}

pub fn should_figterm_launch_exit_status(ctx: &Context, quiet: bool) -> u8 {
    exit_status(ctx, quiet, fig_settings::settings::get_bool_or(ZSH_HOOK_SETTING, false))
}

/// [`should_figterm_launch_exit_status`] with the setting passed in, so tests do not read the
/// settings file of whoever runs them.
fn exit_status(ctx: &Context, quiet: bool, zsh_hook: bool) -> u8 {
    use fig_util::env_var::PROCESS_LAUNCHED_BY_FIG;

    let env = ctx.env();

    if env.get_os(FIG_FORCE_FIGTERM_LAUNCH).is_some() {
        if !quiet {
            writeln!(stdout(), "✅ {FIG_FORCE_FIGTERM_LAUNCH}").ok();
        }
        return 0;
    }

    if env.get_os(FIG_TERM_DISABLED).is_some() {
        if !quiet {
            writeln!(stdout(), "❌ {FIG_TERM_DISABLED}").ok();
        }
        return 1;
    }

    if env.get_os(PROCESS_LAUNCHED_BY_FIG).is_some() {
        if !quiet {
            writeln!(stdout(), "❌ {PROCESS_LAUNCHED_BY_FIG}").ok();
        }
        return 1;
    }

    // Check if inside Emacs
    if env.get_os(INSIDE_EMACS).is_some() {
        if !quiet {
            writeln!(stdout(), "❌ INSIDE_EMACS").ok();
        }
        return 1;
    }

    // Check for Warp Terminal
    if let Ok(term_program) = env.get(TERM_PROGRAM)
        && term_program == WARP_TERMINAL
    {
        if !quiet {
            writeln!(stdout(), "❌ {TERM_PROGRAM} = {WARP_TERMINAL}").ok();
        }
        return 1;
    }

    // Check for SecureCRT
    if let Ok(cf_bundle_identifier) = env.get("__CFBundleIdentifier")
        && cf_bundle_identifier == "com.vandyke.SecureCRT"
    {
        if !quiet {
            writeln!(stdout(), "❌ __CFBundleIdentifier = com.vandyke.SecureCRT").ok();
        }
        return 1;
    }

    // PWSH var is set when launched by `pwsh -Login`, in which case we don't want to init.
    if env.get_os("__PWSH_LOGIN_CHECKED").is_some() {
        if !quiet {
            writeln!(stdout(), "❌ __PWSH_LOGIN_CHECKED").ok();
        }
        return 1;
    }

    // Make sure we're not in CI
    if env.in_ci() {
        if !quiet {
            writeln!(stdout(), "❌ In CI").ok();
        }
        return 1;
    }

    // Don't wrap a shell that was entered over ssh
    if env.in_ssh() {
        if !quiet {
            writeln!(stdout(), "❌ In SSH").ok();
        }
        return 1;
    }

    should_launch(ctx, quiet, zsh_hook)
}

pub fn should_figterm_launch(ctx: &Context) -> ExitCode {
    ExitCode::from(should_figterm_launch_exit_status(ctx, false))
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use std::path::PathBuf;

    use fig_os_shim::process_info::{
        FakePid,
        Pid,
        ProcessInfo,
    };
    use fig_os_shim::{
        ContextBuilder,
        Env,
    };
    use fig_util::env_var::{
        FIG_TERM,
        PROCESS_LAUNCHED_BY_FIG,
    };

    use super::*;

    macro_rules! assert_exit_code {
        ($ctx:expr, $zsh_hook:expr, $exit_code:expr, $msg:expr) => {
            assert_eq!(exit_status(&$ctx, true, $zsh_hook), $exit_code, "{}", $msg);
            assert_eq!(exit_status(&$ctx, false, $zsh_hook), $exit_code, "{}", $msg);
        };
    }

    #[derive(Default, Debug)]
    struct Test {
        name: String,
        env: Option<Env>,
        parent: Option<FakePid>,
        grandparent_exe: Option<PathBuf>,
        grandparent_cmdline: Option<String>,
        zsh_hook: bool,
        expected_exit_code: u8,
    }

    impl Test {
        fn env(mut self, slice: &[(&str, &str)]) -> Self {
            self.env = Some(Env::from_slice(slice));
            self
        }

        fn parent_exe(mut self, exe: impl Into<PathBuf>) -> Self {
            self.parent = Some(FakePid {
                exe: Some(exe.into()),
                ..Default::default()
            });
            self
        }

        fn grandparent_exe(mut self, exe: impl Into<PathBuf>) -> Self {
            self.grandparent_exe = Some(exe.into());
            self
        }

        fn zsh_hook(mut self) -> Self {
            self.zsh_hook = true;
            self
        }

        fn expect(mut self, expected_exit_code: u8) -> Self {
            self.expected_exit_code = expected_exit_code;
            self
        }

        fn run(self) {
            let ctx = {
                let mut ctx = ContextBuilder::new().with_env(self.env.unwrap_or(Env::from_slice(&[])));

                // Create fake ProcessInfo
                let mut current_pid = FakePid::default();
                if let Some(mut parent) = self.parent {
                    if let Some(grandparent_exe) = self.grandparent_exe {
                        let grandparent = FakePid {
                            exe: Some(grandparent_exe),
                            cmdline: self.grandparent_cmdline,
                            ..Default::default()
                        };
                        parent.parent = Some(Box::new(Pid::new_fake(grandparent)));
                    }
                    current_pid.parent = Some(Box::new(Pid::new_fake(parent)));
                }
                ctx = ctx.with_process_info(ProcessInfo::new_fake(current_pid));

                ctx.build()
            };

            assert_exit_code!(ctx, self.zsh_hook, self.expected_exit_code, self.name);
        }
    }

    fn test(name: impl Into<String>) -> Test {
        Test {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Tests for basic override logic where the state of the parent
    /// and grandparent processes don't matter.
    #[test]
    fn override_tests() {
        let tests = [
            test(FIG_FORCE_FIGTERM_LAUNCH)
                .env(&[(FIG_FORCE_FIGTERM_LAUNCH, "1")])
                .expect(0),
            test(FIG_TERM_DISABLED).env(&[(FIG_TERM_DISABLED, "1")]).expect(1),
            test(PROCESS_LAUNCHED_BY_FIG)
                .env(&[(PROCESS_LAUNCHED_BY_FIG, "1")])
                .expect(1),
            test(INSIDE_EMACS).env(&[(INSIDE_EMACS, "1")]).expect(1),
            test(WARP_TERMINAL).env(&[(TERM_PROGRAM, WARP_TERMINAL)]).expect(1),
            test(WARP_TERMINAL).env(&[(TERM_PROGRAM, WARP_TERMINAL)]).expect(1),
            test("In CI").env(&[("CI", "1")]).expect(1),
            test("In ssh").env(&[("SSH_CLIENT", "1")]).expect(1),
            test("__PWSH_LOGIN_CHECKED")
                .env(&[("__PWSH_LOGIN_CHECKED", "1")])
                .expect(1),
            test("SecureCRT")
                .env(&[("__CFBundleIdentifier", "com.vandyke.SecureCRT")])
                .expect(1),
        ];
        for test in tests {
            test.run();
        }
    }

    /// Tests for when the parent process is not valid.
    #[test]
    fn invalid_parent_tests() {
        let tests = [
            test("no parent id").expect(1),
            test("invalid parent").parent_exe("/usr/bin/invalid").expect(1),
            test(format!("In Codespaces with {FIG_TERM}"))
                .parent_exe("/usr/bin/zsh")
                .env(&[("CODESPACES", "1")])
                .env(&[(FIG_TERM, "1")])
                .expect(1),
        ];
        for test in tests {
            test.run();
        }
    }

    /// Tests for when the grandparent process is not valid.
    #[test]
    fn invalid_grandparent_tests() {
        let tests = [
            test("no grandparentparent id").parent_exe("/usr/bin/zsh").expect(1),
            test("on mac with valid parent and invalid grandparent")
                .parent_exe("/usr/bin/zsh")
                .grandparent_exe("/usr/bin/invalid")
                .expect(2),
        ];
        for test in tests {
            test.run();
        }
    }

    /// With the zsh hook on, zsh is never wrapped — even under a terminal that would otherwise
    /// launch the pty binary — while bash is decided as before and an explicit force still
    /// wins.
    #[test]
    fn zsh_hook_tests() {
        let tests = [
            test("zsh with the hook under tmux")
                .parent_exe("/bin/zsh")
                .grandparent_exe("/usr/bin/tmux")
                .zsh_hook()
                .expect(1),
            test("zsh with the hook under Terminal.app")
                .parent_exe("/bin/zsh")
                .grandparent_exe("/usr/bin/invalid")
                .zsh_hook()
                .expect(1),
            test("bash with the zsh hook under tmux")
                .parent_exe("/bin/bash")
                .grandparent_exe("/usr/bin/tmux")
                .zsh_hook()
                .expect(0),
            test("forced with the hook")
                .env(&[(FIG_FORCE_FIGTERM_LAUNCH, "1")])
                .parent_exe("/bin/zsh")
                .zsh_hook()
                .expect(0),
        ];
        for test in tests {
            test.run();
        }
    }

    /// Tests for when the app should launch, ignoring any of the overrides
    /// captured under override_tests below.
    #[test]
    fn should_launch_tests() {
        let tests = [
            test("on mac with valid parent and special grandparent")
                .parent_exe("/usr/bin/zsh")
                .grandparent_exe("/usr/bin/tmux")
                .expect(0),
            test(format!("In Codespaces without {FIG_TERM}"))
                .parent_exe("/usr/bin/zsh")
                .env(&[("CODESPACES", "1")])
                .expect(0),
        ];
        for test in tests {
            test.run();
        }
    }
}
