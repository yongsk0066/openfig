//! Which keys a hook session sends to the desktop instead of zsh.
//!
//! A hook session only sees the keys `fig.zsh` binds, and it has to know *before* the key is
//! pressed whether to forward it, so the decision is made here for each of those keys up front and
//! pushed to the shell as `INTERCEPT` whenever the answer changes.

use std::collections::{
    BTreeSet,
    HashMap,
};
use std::sync::LazyLock;

use fig_proto::figterm::Action;
use fig_settings::keybindings::{
    KeyBinding,
    KeyBindings,
};

/// The keys `fig.zsh` binds (`_fig_names`), by the name a `K` line carries.
pub const SHELL_KEYS: &[&str] = &[
    "tab",
    "shift+tab",
    "up",
    "down",
    "enter",
    "esc",
    "control+j",
    "control+k",
    "control+n",
    "control+p",
    "control+r",
];

// Same lists as the pty binary's interceptor.
const GLOBAL_ACTIONS: &[&str] = &["toggleAutocomplete", "showAutocomplete"];
const IGNORE_ACTION: &str = "ignore";

/// Actions the autocomplete app can answer with an insertion (`hooks/keypress.ts`):
/// `showAutocomplete` inserts the only suggestion when `autocomplete.onlyShowOnTab` is on.
const INSERTING_ACTIONS: &[&str] = &[
    "insertSelected",
    "insertCommonPrefix",
    "insertCommonPrefixOrNavigateDown",
    "insertCommonPrefixOrInsertSelected",
    "insertSelectedAndExecute",
    "execute",
    "showAutocomplete",
];

/// Whether the shell should wait for an insertion after `action` (`KEY 1` rather than `KEY 2`).
/// Waiting after a navigation key would hold every ↑/↓ for the whole window.
pub fn may_insert(action: &str) -> bool {
    INSERTING_ACTIONS.contains(&action)
}

static ONLY_SHOW_ON_TAB: LazyLock<bool> =
    LazyLock::new(|| fig_settings::settings::get_bool_or("autocomplete.onlyShowOnTab", false));

/// A keybinding (`"ctrl+shift+Tab"`, `"arrowup"`) spelled the way [`SHELL_KEYS`] are, or `None`
/// for text the pty binary's `key_from_text` would not accept either.
pub fn key_name(binding: &str) -> Option<String> {
    let (mut control, mut alt, mut shift, mut meta) = (false, false, false, false);
    let mut rest = binding;
    let key = loop {
        match rest.split_once('+') {
            Some(("", "")) | None => break rest,
            Some((modifier, key)) => {
                match modifier {
                    "ctrl" | "control" => control = true,
                    "shift" => shift = true,
                    "alt" | "option" => alt = true,
                    "meta" | "command" => meta = true,
                    _ => {},
                }
                rest = key;
            },
        }
    };
    let key = match key {
        "arrowup" => "up".to_owned(),
        "arrowdown" => "down".to_owned(),
        "arrowleft" => "left".to_owned(),
        "arrowright" => "right".to_owned(),
        "backspace" | "enter" | "up" | "down" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" | "tab"
        | "delete" | "insert" | "esc" => key.to_owned(),
        f_key if f_key.starts_with('f') && f_key[1..].parse::<u8>().is_ok() => f_key.to_owned(),
        c => {
            let mut chars = c.chars();
            let mut first = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            // As `key_from_text`: `shift+a` is `A`, and with ctrl or alt both cases are one key.
            if shift && first.is_ascii_lowercase() {
                first = first.to_ascii_uppercase();
                shift = false;
            }
            if control || alt {
                first = first.to_ascii_lowercase();
            }
            first.to_string()
        },
    };
    let mut name = String::new();
    for (on, modifier) in [(control, "control+"), (alt, "alt+"), (shift, "shift+"), (meta, "meta+")] {
        if on {
            name.push_str(modifier);
        }
    }
    name.push_str(&key);
    Some(name)
}

/// The pty binary's `KeyInterceptor`, keyed by name.
#[derive(Debug, Clone)]
pub struct Interceptor {
    intercept: bool,
    intercept_global: bool,
    window_visible: bool,
    mappings: HashMap<String, String>,
}

impl Default for Interceptor {
    /// The built-in bindings, as the pty binary starts with.
    fn default() -> Self {
        let mut interceptor = Interceptor {
            intercept: false,
            intercept_global: false,
            window_visible: false,
            mappings: HashMap::new(),
        };
        for KeyBinding { identifier, binding } in KeyBindings::load_hardcoded() {
            interceptor.insert(&binding, identifier);
        }
        interceptor
    }
}

impl Interceptor {
    fn insert(&mut self, binding: &str, identifier: String) {
        if let Some(name) = key_name(binding) {
            self.mappings.insert(name, identifier);
        }
    }

    pub fn set_intercept(&mut self, intercept: bool) {
        self.intercept = intercept;
    }

    pub fn set_intercept_global(&mut self, intercept_global: bool) {
        self.intercept_global = intercept_global;
    }

    pub fn set_window_visible(&mut self, window_visible: bool) {
        self.window_visible = window_visible;
    }

    pub fn set_actions(&mut self, actions: &[Action], override_actions: bool) {
        if override_actions {
            self.mappings.clear();
        }
        for Action { identifier, bindings } in actions {
            for binding in bindings {
                self.insert(binding, identifier.clone());
            }
        }
    }

    /// Escape does this in the pty binary, and so does a command starting.
    pub fn reset(&mut self) {
        self.intercept = false;
        self.intercept_global = false;
    }

    /// The action `key` triggers right now, if it is intercepted at all.
    pub fn action(&self, key: &str) -> Option<&str> {
        match (self.intercept_global, self.intercept) {
            (true, false) => {
                if key == "tab" && *ONLY_SHOW_ON_TAB {
                    Some("showAutocomplete")
                } else {
                    match self.mappings.get(key).map(String::as_str) {
                        Some(action) if action != IGNORE_ACTION && GLOBAL_ACTIONS.contains(&action) => Some(action),
                        _ => None,
                    }
                }
            },
            (_, true) if self.window_visible => match self.mappings.get(key).map(String::as_str) {
                Some(IGNORE_ACTION) => None,
                other => other,
            },
            _ => None,
        }
    }

    /// The keys the shell should forward, i.e. the argument of `INTERCEPT`.
    pub fn forwarded(&self) -> BTreeSet<String> {
        SHELL_KEYS
            .iter()
            .filter(|key| self.action(key).is_some())
            .map(|key| (*key).to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_named_like_the_shell_keys() {
        assert_eq!(key_name("tab").as_deref(), Some("tab"));
        assert_eq!(key_name("shift+tab").as_deref(), Some("shift+tab"));
        assert_eq!(key_name("arrowup").as_deref(), Some("up"));
        assert_eq!(key_name("control+k").as_deref(), Some("control+k"));
        assert_eq!(key_name("ctrl+K").as_deref(), Some("control+k"));
        assert_eq!(key_name("shift+ctrl+r").as_deref(), Some("control+r"));
        assert_eq!(key_name("shift+a").as_deref(), Some("A"));
        assert_eq!(key_name("command+i").as_deref(), Some("meta+i"));
        assert_eq!(key_name("control+="), Some("control+=".into()));
        assert_eq!(key_name("f12").as_deref(), Some("f12"));
        assert_eq!(key_name("invalid"), None);
    }

    /// Every default binding the shell can deliver is one `fig.zsh` binds, so none is silently
    /// unreachable on the hook path.
    #[test]
    fn the_shell_binds_every_deliverable_default() {
        let interceptor = Interceptor::default();
        for key in [
            "tab",
            "shift+tab",
            "up",
            "down",
            "enter",
            "esc",
            "control+j",
            "control+k",
            "control+n",
            "control+p",
            "control+r",
        ] {
            assert!(interceptor.mappings.contains_key(key), "{key} has no default action");
            assert!(SHELL_KEYS.contains(&key));
        }
    }

    #[test]
    fn keys_are_forwarded_only_while_the_window_shows() {
        let mut interceptor = Interceptor::default();
        assert!(interceptor.forwarded().is_empty());

        interceptor.set_intercept(true);
        assert!(
            interceptor.forwarded().is_empty(),
            "intercepting but the window is hidden"
        );

        interceptor.set_window_visible(true);
        let forwarded = interceptor.forwarded();
        assert!(forwarded.contains("tab") && forwarded.contains("down") && forwarded.contains("enter"));
        assert_eq!(interceptor.action("tab"), Some("insertCommonPrefix"));
        assert_eq!(interceptor.action("down"), Some("navigateDown"));

        // The desktop's own list replaces the built-in one.
        interceptor.set_actions(
            &[Action {
                identifier: "navigateDown".into(),
                bindings: vec!["tab".into()],
            }],
            true,
        );
        assert_eq!(interceptor.forwarded(), BTreeSet::from(["tab".to_owned()]));

        interceptor.set_actions(
            &[Action {
                identifier: IGNORE_ACTION.into(),
                bindings: vec!["tab".into()],
            }],
            false,
        );
        assert!(interceptor.forwarded().is_empty(), "ignore wins");

        interceptor.reset();
        assert_eq!(interceptor.action("tab"), None);
    }

    #[test]
    fn only_inserting_actions_make_the_shell_wait() {
        let interceptor = Interceptor::default();
        let waits = |key| interceptor.mappings.get(key).is_some_and(|action| may_insert(action));
        assert!(waits("tab") && waits("enter"));
        assert!(!waits("up") && !waits("down") && !waits("esc") && !waits("control+r"));
    }

    #[test]
    fn global_interception_forwards_only_global_actions() {
        let mut interceptor = Interceptor::default();
        interceptor.set_actions(
            &[
                Action {
                    identifier: "toggleAutocomplete".into(),
                    bindings: vec!["control+k".into()],
                },
                Action {
                    identifier: "navigateDown".into(),
                    bindings: vec!["down".into()],
                },
            ],
            true,
        );
        interceptor.set_intercept_global(true);
        assert_eq!(interceptor.action("control+k"), Some("toggleAutocomplete"));
        assert_eq!(interceptor.action("down"), None);
    }
}
