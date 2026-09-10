use std::borrow::Cow;
use std::fmt;
use std::sync::OnceLock;

use fig_os_shim::Context;
use serde::{
    Deserialize,
    Serialize,
};
use strum::{
    EnumIter,
    IntoEnumIterator,
};

/// Terminals that macOS supports
pub const MACOS_TERMINALS: &[Terminal] = &[
    Terminal::Iterm,
    Terminal::Tabby,
    Terminal::TerminalApp,
    Terminal::VSCodeInsiders,
    Terminal::VSCode,
    Terminal::VSCodium,
    Terminal::Cursor,
    Terminal::CursorNightly,
    Terminal::Windsurf,
    Terminal::WindsurfNext,
    Terminal::Positron,
    Terminal::Trae,
];

/// Other terminals that figterm should launch within that are not full terminal emulators
pub const SPECIAL_TERMINALS: &[Terminal] = &[Terminal::Tmux, Terminal::Nvim, Terminal::Vim, Terminal::Zellij];

/// Bundle ids of every [`Terminal`] that renders its UI in a webview ([`Terminal::is_xterm`])
/// and therefore needs `AXManualAccessibility` switched on before the caret can be read.
///
/// Derived from the enum rather than written out, so it cannot drift from
/// [`Terminal::is_xterm`]. `macos-utils` owns the window server but cannot depend on this
/// crate (cycle), so `fig_desktop` injects this list into `WindowServer::new`.
pub fn xterm_bundle_ids() -> &'static [&'static str] {
    static IDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    IDS.get_or_init(|| {
        Terminal::iter()
            .filter(Terminal::is_xterm)
            .filter_map(|terminal| match terminal.to_bundle_id() {
                // every `to_bundle_id` arm is a string literal, so this never drops one
                Some(Cow::Borrowed(id)) => Some(id),
                Some(Cow::Owned(_)) | None => None,
            })
            .collect()
    })
}

pub fn current_terminal() -> Option<&'static Terminal> {
    static CURRENT_TERMINAL: OnceLock<Option<Terminal>> = OnceLock::new();
    CURRENT_TERMINAL
        .get_or_init(|| Terminal::parent_terminal(&Context::new()))
        .as_ref()
}

pub fn current_terminal_version() -> Option<&'static str> {
    static CURRENT_TERMINAL_VERSION: OnceLock<Option<String>> = OnceLock::new();
    CURRENT_TERMINAL_VERSION.get_or_init(Terminal::version).as_deref()
}

/// Checks if the current process is inside of one of the pseudoterminals listed under
/// [`SPECIAL_TERMINALS`], returning the terminal if true.
pub fn in_special_terminal(ctx: &Context) -> Option<Terminal> {
    Terminal::from_process_info(ctx, &SPECIAL_TERMINALS.to_vec())
}

/// All supported terminals
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter)]
#[serde(rename_all = "kebab-case")]
pub enum Terminal {
    /// iTerm 2
    Iterm,
    /// Native macOS terminal
    TerminalApp,
    /// Hyper terminal
    Hyper,
    /// VSCode terminal
    VSCode,
    /// VSCode Insiders
    VSCodeInsiders,
    /// VSCodium
    VSCodium,
    /// Tabby
    Tabby,
    /// Cursor
    Cursor,
    /// Cursor Nightly
    CursorNightly,

    // Other pseudoterminal that we want to launch within
    /// Tmux
    Tmux,
    /// Vim
    Vim,
    /// Nvim
    Nvim,
    /// Zellij
    Zellij,
    /// Windsurf
    Windsurf,
    /// Windsurf Next
    WindsurfNext,
    /// Positron
    Positron,
    /// Trae
    Trae,
}

impl fmt::Display for Terminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Terminal::Iterm => write!(f, "iTerm 2"),
            Terminal::TerminalApp => write!(f, "macOS"),
            Terminal::Hyper => write!(f, "Hyper"),
            Terminal::VSCode => write!(f, "VSCode"),
            Terminal::VSCodeInsiders => write!(f, "VSCode Insiders"),
            Terminal::VSCodium => write!(f, "VSCodium"),
            Terminal::Tabby => write!(f, "Tabby"),
            Terminal::Tmux => write!(f, "Tmux"),
            Terminal::Vim => write!(f, "Vim"),
            Terminal::Nvim => write!(f, "Nvim"),
            Terminal::Zellij => write!(f, "Zellij"),
            Terminal::Cursor => write!(f, "Cursor"),
            Terminal::CursorNightly => write!(f, "Cursor Nightly"),
            Terminal::Windsurf => write!(f, "Windsurf"),
            Terminal::WindsurfNext => write!(f, "Windsurf Next"),
            Terminal::Positron => write!(f, "Positron"),
            Terminal::Trae => write!(f, "Trae"),
        }
    }
}

impl Terminal {
    /// Attempts to return the suspected terminal emulator for the current process.
    ///
    /// Note that "special" pseudoterminals like tmux will not be returned.
    pub fn parent_terminal(ctx: &Context) -> Option<Self> {
        let env = ctx.env();

        if let Ok(bundle_id) = env.get("__CFBundleIdentifier") {
            if let Some(term) = Self::from_bundle_id(bundle_id) {
                return Some(term);
            }
        }

        match env.get("TERM_PROGRAM").ok().as_deref() {
            Some("iTerm.app") => return Some(Terminal::Iterm),
            Some("Apple_Terminal") => return Some(Terminal::TerminalApp),
            Some("Hyper") => return Some(Terminal::Hyper),
            Some("vscode") => match std::env::var("TERM_PROGRAM_VERSION").ok().as_deref() {
                Some(v) if v.contains("insiders") => return Some(Terminal::VSCodeInsiders),
                _ => return Some(Terminal::VSCode),
            },
            Some("Tabby") => return Some(Terminal::Tabby),
            _ => (),
        };

        Self::from_process_info(ctx, &MACOS_TERMINALS.to_vec())
    }

    /// Attempts to return the suspected terminal emulator for the current process according to the
    /// process hierarchy. Only the list provided in `terminals` will be searched for.
    pub fn from_process_info(ctx: &Context, terminals: &Vec<Terminal>) -> Option<Self> {
        let mut option_pid = Some(Box::new(ctx.process_info().current_pid()));
        let (mut curr_depth, max_depth) = (0, 5);
        while curr_depth < max_depth {
            if let Some(pid) = option_pid {
                if let Some(exe) = pid.exe() {
                    if let Some(name) = exe.file_name().and_then(|s| s.to_str()) {
                        for terminal in terminals {
                            if terminal.executable_names().contains(&name) {
                                return Some(terminal.clone());
                            }
                        }
                    }
                }
                if let Some(cmdline) = pid.cmdline() {
                    if let Some(terminal) = Self::try_from_cmdline(&cmdline, terminals) {
                        return Some(terminal.clone());
                    }
                }
                option_pid = pid.parent();
                curr_depth += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Attempts to find the suspected terminal according to the provided `cmdline` - ie,
    /// the value from /proc/pid/cmdline except with null bytes replaced with space.
    ///
    /// Only the list provided in `terminals` will be searched for.
    pub fn try_from_cmdline(cmdline: &str, terminals: &Vec<Terminal>) -> Option<Self> {
        // Default logic that checks the final path element of the first argument.
        let first_arg_name = cmdline
            .split(' ')
            .take(1)
            .next()
            .and_then(|cmd| cmd.split('/').next_back())
            .map(str::to_string);
        if let Some(first_arg_name) = first_arg_name {
            for terminal in terminals {
                if terminal.executable_names().contains(&first_arg_name.as_str()) {
                    return Some(terminal.clone());
                }
            }
        }

        None
    }

    pub fn version() -> Option<String> {
        static RE: OnceLock<Option<regex::Regex>> = OnceLock::new();
        let re = RE.get_or_init(|| regex::Regex::new("[0-9\\-\\._]+").ok()).as_ref()?;
        let version = std::env::var("TERM_PROGRAM_VERSION").ok()?;
        match re.captures(&version).is_some() {
            true => Some(version),
            false => None,
        }
    }

    pub fn internal_id(&self) -> Cow<'static, str> {
        match self {
            Terminal::Iterm => "iterm".into(),
            Terminal::TerminalApp => "terminal".into(),
            Terminal::Hyper => "hyper".into(),
            Terminal::VSCode => "vscode".into(),
            Terminal::VSCodeInsiders => "vscode-insiders".into(),
            Terminal::VSCodium => "vscodium".into(),
            Terminal::Tabby => "tabby".into(),
            Terminal::Tmux => "tmux".into(),
            Terminal::Vim => "vim".into(),
            Terminal::Nvim => "nvim".into(),
            Terminal::Zellij => "zellij".into(),
            Terminal::Cursor => "cursor".into(),
            Terminal::CursorNightly => "cursor-nightly".into(),
            Terminal::Windsurf => "windsurf".into(),
            Terminal::WindsurfNext => "windsurf-next".into(),
            Terminal::Positron => "positron".into(),
            Terminal::Trae => "trae".into(),
        }
    }

    /// Get the bundle identifier for the terminal
    /// Note: this does not gracefully handle terminals that have changed bundle identifiers
    /// recently such as VSCodium. We default to the current identifier.
    pub fn to_bundle_id(&self) -> Option<Cow<'static, str>> {
        match self {
            Terminal::Iterm => Some("com.googlecode.iterm2".into()),
            Terminal::TerminalApp => Some("com.apple.Terminal".into()),
            Terminal::Hyper => Some("co.zeit.hyper".into()),
            Terminal::VSCode => Some("com.microsoft.VSCode".into()),
            Terminal::VSCodeInsiders => Some("com.microsoft.VSCodeInsiders".into()),
            Terminal::VSCodium => Some("com.vscodium".into()),
            Terminal::Tabby => Some("org.tabby".into()),
            Terminal::Cursor => Some("com.todesktop.230313mzl4w4u92".into()),
            Terminal::CursorNightly => Some("com.todesktop.23052492jqa5xjo".into()),
            Terminal::Windsurf => Some("com.exafunction.windsurf".into()),
            Terminal::WindsurfNext => Some("com.exafunction.windsurf-next".into()),
            Terminal::Positron => Some("co.posit.positron".into()),
            Terminal::Trae => Some("com.trae.app".into()),
            _ => None,
        }
    }

    pub fn from_bundle_id(bundle: impl AsRef<str>) -> Option<Self> {
        let bundle = bundle.as_ref();
        let res = match bundle {
            "com.googlecode.iterm2" => Terminal::Iterm,
            "com.apple.Terminal" => Terminal::TerminalApp,
            "co.zeit.hyper" => Terminal::Hyper,
            "com.microsoft.VSCode" => Terminal::VSCode,
            "com.microsoft.VSCodeInsiders" => Terminal::VSCodeInsiders,
            "com.vscodium" | "com.visualstudio.code.oss" => Terminal::VSCodium,
            "org.tabby" => Terminal::Tabby,
            "com.todesktop.230313mzl4w4u92" => Terminal::Cursor,
            "com.todesktop.23052492jqa5xjo" => Terminal::CursorNightly,
            "com.exafunction.windsurf" => Terminal::Windsurf,
            "com.exafunction.windsurf-next" => Terminal::WindsurfNext,
            "co.posit.positron" => Terminal::Positron,
            "com.trae.app" => Terminal::Trae,
            _ => return None,
        };

        Some(res)
    }

    pub fn is_xterm(&self) -> bool {
        matches!(
            self,
            Terminal::VSCode
                | Terminal::VSCodeInsiders
                | Terminal::VSCodium
                | Terminal::Hyper
                | Terminal::Tabby
                | Terminal::Cursor
                | Terminal::CursorNightly
                | Terminal::Windsurf
                | Terminal::WindsurfNext
                | Terminal::Trae
        )
    }

    pub fn executable_names(&self) -> &'static [&'static str] {
        match self {
            Terminal::VSCode => &["code"],
            Terminal::VSCodeInsiders => &["code-insiders"],
            Terminal::Hyper => &["hyper"],
            Terminal::Tabby => &["tabby"],
            Terminal::Cursor => &["Cursor", "cursor"],
            Terminal::CursorNightly => &["Cursor Nightly", "cursor-nightly"],
            Terminal::Windsurf => &["windsurf"],
            Terminal::WindsurfNext => &["windsurf-next"],
            Terminal::Positron => &["positron"],
            Terminal::Trae => &["trae"],

            Terminal::Tmux => &["tmux", "tmux: server"],
            Terminal::Vim => &["vim"],
            Terminal::Nvim => &["nvim"],
            Terminal::Zellij => &["zellij"],

            _ => &[],
        }
    }

    pub fn supports_fancy_boxes(&self) -> bool {
        !matches!(
            self,
            Terminal::VSCode
                | Terminal::VSCodeInsiders
                | Terminal::VSCodium
                | Terminal::Cursor
                | Terminal::CursorNightly
                | Terminal::Windsurf
                | Terminal::WindsurfNext
                | Terminal::Trae
        )
    }

    /// Other pseudoterminal that we want to launch within
    pub fn is_special(&self) -> bool {
        matches!(self, Terminal::Tmux | Terminal::Vim | Terminal::Nvim | Terminal::Zellij)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_process_info() {
        Terminal::from_process_info(&Context::new(), &MACOS_TERMINALS.to_vec());
    }

    #[test]
    fn test_xterm_bundle_ids() {
        let ids = xterm_bundle_ids();
        // every xterm terminal has a bundle id, so nothing is silently dropped
        assert_eq!(ids.len(), Terminal::iter().filter(Terminal::is_xterm).count());
        for id in ids {
            assert!(Terminal::from_bundle_id(id).is_some_and(|t| t.is_xterm()));
        }
    }
}
