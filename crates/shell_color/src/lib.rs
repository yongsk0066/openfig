//! Parsing for the terminal colour specifications figterm sees at runtime.
//!
//! This is a dependency-free reimplementation of the parts of the `shell-color` crate (1.0.1,
//! MIT OR Apache-2.0, <https://codeberg.org/chaynabors/shell-color>) that this workspace uses:
//! [`get_color_support`] and [`parse_suggestion_color_zsh_autosuggest`], plus the [`ColorSupport`],
//! [`SuggestionColor`] and [`VTermColor`] types they exchange with `alacritty_terminal` and
//! `figterm`. The upstream crate pulled in `nu-color-config` for a nushell entry point we never
//! called, and with it ~20 `nu-*` crates, `chrono`, `miette`, `sysinfo` and a second `nix`; only
//! the zsh path is kept here, so the crate has no dependencies at all.
//!
//! The colour-matching tables and the parsing rules are fish's, which is where zsh-autosuggestions
//! support in figterm originally came from; behaviour is intentionally identical to 1.0.1,
//! including its quirks (see [`parse_suggestion_color_zsh_autosuggest`]).

use std::fmt;
use std::ops::{
    BitOr,
    BitOrAssign,
};

/// A foreground/background colour pair, as parsed out of a shell's suggestion-style setting.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SuggestionColor {
    /// Foreground colour, if the spec set one.
    pub fg: Option<VTermColor>,
    /// Background colour, if the spec set one.
    pub bg: Option<VTermColor>,
}

impl SuggestionColor {
    /// The foreground colour, if the spec set one.
    pub fn fg(&self) -> Option<VTermColor> {
        self.fg.clone()
    }

    /// The background colour, if the spec set one.
    pub fn bg(&self) -> Option<VTermColor> {
        self.bg.clone()
    }
}

/// A colour as libvterm models it: either true colour or an index into the terminal palette.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VTermColor {
    /// A 24-bit colour.
    Rgb {
        /// Red channel.
        red: u8,
        /// Green channel.
        green: u8,
        /// Blue channel.
        blue: u8,
    },
    /// An index into the terminal's palette.
    Indexed {
        /// Palette index.
        idx: u8,
    },
}

impl VTermColor {
    /// A palette index.
    pub const fn from_idx(idx: u8) -> Self {
        VTermColor::Indexed { idx }
    }

    /// A 24-bit colour.
    pub const fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        VTermColor::Rgb { red, green, blue }
    }
}

/// What the terminal we are attached to can render.
///
/// A bit set; the two flags are independent, and "neither" means a plain 16-colour terminal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ColorSupport(u32);

impl ColorSupport {
    /// The terminal understands 24-bit true colour.
    pub const TERM24BIT: Self = Self(1 << 2);
    /// The terminal understands the 256-colour palette.
    pub const TERM256: Self = Self(1 << 1);

    /// No colour support beyond the base 16 colours.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Whether no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every flag in `other` is set here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for ColorSupport {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ColorSupport {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for ColorSupport {
    /// Matches what `bitflags` 2.x rendered for the upstream type, so log output is unchanged.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColorSupport(")?;
        let mut empty = true;
        for (name, flag) in [("TERM256", Self::TERM256), ("TERM24BIT", Self::TERM24BIT)] {
            if self.contains(flag) {
                if !empty {
                    f.write_str(" | ")?;
                }
                f.write_str(name)?;
                empty = false;
            }
        }
        if empty {
            f.write_str("0x0")?;
        }
        f.write_str(")")
    }
}

/// Work out what the terminal supports from the environment.
///
/// This is fish's inference, transcribed: `$fish_term256`/`$fish_term24bit` win outright, otherwise
/// `$TERM`, `$COLORTERM`, `$ITERM_SESSION_ID`, `$KONSOLE_*` and `$VTE_VERSION` are consulted in a
/// fixed order — the first of those that is *set* decides, even if its value says "no".
pub fn get_color_support() -> ColorSupport {
    color_support_from_env(|key| std::env::var(key).ok())
}

// The env chain below has branches whose bodies coincide; they are kept apart because they are
// distinct rules, and upstream wrote them the same way.
#[allow(clippy::if_same_then_else)]
fn color_support_from_env(env: impl Fn(&str) -> Option<String>) -> ColorSupport {
    let mut support_term256 = false;
    let mut support_term24bit = false;

    let term = env("TERM");

    // Detect or infer term256 support. If fish_term256 is set, we respect it; otherwise infer it
    // from the TERM variable.
    if let Some(fish_term256) = env("fish_term256") {
        support_term256 = bool_from_string(&fish_term256);
    } else if term.as_deref().is_some_and(|term| term.contains("256color")) {
        support_term256 = true;
    } else if term.as_deref().is_some_and(|term| term.contains("xterm")) {
        // Assume that all 'xterm's can handle 256, except for Terminal.app from Snow Leopard.
        if env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal") {
            let tpv = env("TERM_PROGRAM_VERSION").and_then(|tpv| tpv.parse::<i32>().ok());
            if tpv.unwrap_or(0) > 299 {
                support_term256 = true;
            }
        } else {
            support_term256 = true;
        }
    }

    if let Some(fish_term24bit) = env("fish_term24bit") {
        support_term24bit = bool_from_string(&fish_term24bit);
    } else if env("STY").is_some() || term.as_deref().is_some_and(|term| term.starts_with("eterm")) {
        // Screen and emacs' ansi-term swallow truecolor sequences, so we ignore them unless
        // force-enabled.
        support_term24bit = false;
    } else if let Some(ct) = env("COLORTERM") {
        // If someone set $COLORTERM, that's the sort of color they want.
        if ct == "truecolor" || ct == "24bit" {
            support_term24bit = true;
        }
    } else if env("KONSOLE_VERSION").is_some() || env("KONSOLE_PROFILE_NAME").is_some() {
        // All konsole versions that use $KONSOLE_VERSION are new enough to support this, so no
        // check is necessary.
        support_term24bit = true;
    } else if let Some(it) = env("ITERM_SESSION_ID") {
        // Supporting versions of iTerm include a colon here. We assume that if this is iTerm, it
        // can't also be st, so having this check inside is okay.
        if it.contains(':') {
            support_term24bit = true;
        }
    } else if term.as_deref().is_some_and(|term| term.starts_with("st-")) {
        support_term24bit = true;
    } else if env("VTE_VERSION").and_then(|vte| vte.parse::<i32>().ok()).unwrap_or(0) > 3600 {
        support_term24bit = true;
    }

    let mut support = ColorSupport::empty();
    if support_term256 {
        support |= ColorSupport::TERM256;
    }
    if support_term24bit {
        support |= ColorSupport::TERM24BIT;
    }
    support
}

/// Parse a `ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE`-shaped spec, e.g. `fg=8`, `fg=#ff0000,bold`,
/// `fg=cyan,underline,bg=black`.
///
/// Comma-separated segments; only those prefixed `fg=` or `bg=` are looked at, so attribute
/// segments (`bold`, `underline`, `standout`, ...) are skipped — figterm only ever needed the
/// colours. Within a segment a fish colour name is tried first, then `#rrggbb`/`#rgb`, then a
/// palette index.
///
/// Two upstream quirks are kept deliberately, because the OSC 697 producers rely on the resulting
/// values: an unparseable colour (`fg=nonsense`, and also a bare `fg=ffffff` with no `#`, which the
/// zsh path never tries as hex) falls back to index 0 rather than being ignored, and an index is
/// only accepted if it is below 16 on a terminal with no colour support, or below 256 otherwise.
/// A later segment for the same channel overwrites an earlier one.
pub fn parse_suggestion_color_zsh_autosuggest(suggestion_str: &str, color_support: ColorSupport) -> SuggestionColor {
    let mut sc = SuggestionColor { fg: None, bg: None };

    for segment in suggestion_str.split(',') {
        let (is_fg, color_name) = match segment.split_at_checked(3) {
            Some(("fg=", rest)) => (true, rest),
            Some(("bg=", rest)) => (false, rest),
            _ => continue,
        };

        // Named colors are parsed with fish's table. This can fail in two ways: false positives
        // from fish colors that aren't supported in zsh (e.g. brblack), and false negatives that
        // aren't supported in fish (e.g. abbreviations like bl for black).
        let mut color = try_parse_named(color_name);
        if color.is_none() && color_name.starts_with('#') {
            color = try_parse_rgb(color_name);
        }

        let vc = match color {
            Some(color) => Some(color_to_vterm_color(color, color_support)),
            None => {
                // Custom zsh logic - try 256 indexed colors.
                let index = color_name.parse::<u64>().unwrap_or(0);
                let index_supported = if color_support.is_empty() {
                    index < 16
                } else {
                    index < 256
                };
                index_supported.then_some(VTermColor::Indexed { idx: index as u8 })
            },
        };

        // A rejected index leaves the previous value in place, as upstream did.
        if let Some(vc) = vc {
            if is_fg {
                sc.fg = Some(vc);
            } else {
                sc.bg = Some(vc);
            }
        }
    }

    sc
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Color {
    /// A fish colour name, already resolved to its palette index.
    Named(u8),
    /// A literal 24-bit colour.
    Rgb([u8; 3]),
}

fn bool_from_string(x: &str) -> bool {
    match x.chars().next() {
        Some(first) => "YTyt1".contains(first),
        None => false,
    }
}

const fn squared_difference(p1: i64, p2: i64) -> u64 {
    let diff = (p1 - p2).unsigned_abs();
    diff * diff
}

/// Index of the palette entry closest to `rgb` in (naive, non-perceptual) RGB space. Ties go to
/// the last candidate, as upstream.
const fn convert_color(rgb: [u8; 3], colors: &[u32]) -> u8 {
    let r = rgb[0] as i64;
    let g = rgb[1] as i64;
    let b = rgb[2] as i64;

    let mut best_distance = u64::MAX;
    let mut best_index = u8::MAX;

    let mut i = 0;
    while i < colors.len() {
        let color = colors[i];
        let test_r = ((color >> 16) & 0xff) as i64;
        let test_g = ((color >> 8) & 0xff) as i64;
        let test_b = (color & 0xff) as i64;
        let distance = squared_difference(r, test_r) + squared_difference(g, test_g) + squared_difference(b, test_b);
        if distance <= best_distance {
            best_index = i as u8;
            best_distance = distance;
        }
        i += 1;
    }

    best_index
}

/// Parse the rgb formats fish accepts, case insensitively: `#FA3`, `#F3A035`, `FA3`, `F3A035`.
fn try_parse_rgb(name: &str) -> Option<Color> {
    let name = name.strip_prefix('#').unwrap_or(name).as_bytes();
    let mut rgb = [0; 3];

    match name.len() {
        // Format: FA3
        3 => {
            for (channel, c) in rgb.iter_mut().zip(name) {
                let val = char::from(*c).to_digit(16)? as u8;
                *channel = val * 16 + val;
            }
        },
        // Format: F3A035
        6 => {
            for (i, channel) in rgb.iter_mut().enumerate() {
                let val_hi = char::from(name[i * 2]).to_digit(16)? as u8;
                let val_low = char::from(name[i * 2 + 1]).to_digit(16)? as u8;
                *channel = val_hi * 16 + val_low;
            }
        },
        _ => return None,
    }

    Some(Color::Rgb(rgb))
}

/// fish's named colours and the palette index each resolves to. Kept sorted by name.
static NAMED_COLORS: &[(&str, u8)] = &[
    ("black", 0),
    ("blue", 4),
    ("brblack", 8),
    ("brblue", 12),
    ("brbrown", 11),
    ("brcyan", 14),
    ("brgreen", 10),
    ("brgrey", 8),
    ("brmagenta", 13),
    ("brown", 3),
    ("brpurple", 13),
    ("brred", 9),
    ("brwhite", 15),
    ("bryellow", 11),
    ("cyan", 6),
    ("green", 2),
    ("grey", 7),
    ("magenta", 5),
    ("purple", 5),
    ("red", 1),
    ("white", 7),
    ("yellow", 3),
];

fn try_parse_named(s: &str) -> Option<Color> {
    let s = s.to_ascii_lowercase();
    let idx = NAMED_COLORS.binary_search_by(|(name, _)| (*name).cmp(&*s)).ok()?;
    Some(Color::Named(NAMED_COLORS[idx].1))
}

const fn term16_color_for_rgb(rgb: [u8; 3]) -> u8 {
    const K_COLORS: &[u32] = &[
        0x000000, // Black
        0x800000, // Red
        0x008000, // Green
        0x808000, // Yellow
        0x000080, // Blue
        0x800080, // Magenta
        0x008080, // Cyan
        0xc0c0c0, // White
        0x808080, // Bright Black
        0xff0000, // Bright Red
        0x00ff00, // Bright Green
        0xffff00, // Bright Yellow
        0x0000ff, // Bright Blue
        0xff00ff, // Bright Magenta
        0x00ffff, // Bright Cyan
        0xffffff, // Bright White
    ];
    convert_color(rgb, K_COLORS)
}

const fn term256_color_for_rgb(rgb: [u8; 3]) -> u8 {
    const K_COLORS: &[u32] = &[
        0x000000, 0x00005f, 0x000087, 0x0000af, 0x0000d7, 0x0000ff, 0x005f00, 0x005f5f, 0x005f87, 0x005faf, 0x005fd7,
        0x005fff, 0x008700, 0x00875f, 0x008787, 0x0087af, 0x0087d7, 0x0087ff, 0x00af00, 0x00af5f, 0x00af87, 0x00afaf,
        0x00afd7, 0x00afff, 0x00d700, 0x00d75f, 0x00d787, 0x00d7af, 0x00d7d7, 0x00d7ff, 0x00ff00, 0x00ff5f, 0x00ff87,
        0x00ffaf, 0x00ffd7, 0x00ffff, 0x5f0000, 0x5f005f, 0x5f0087, 0x5f00af, 0x5f00d7, 0x5f00ff, 0x5f5f00, 0x5f5f5f,
        0x5f5f87, 0x5f5faf, 0x5f5fd7, 0x5f5fff, 0x5f8700, 0x5f875f, 0x5f8787, 0x5f87af, 0x5f87d7, 0x5f87ff, 0x5faf00,
        0x5faf5f, 0x5faf87, 0x5fafaf, 0x5fafd7, 0x5fafff, 0x5fd700, 0x5fd75f, 0x5fd787, 0x5fd7af, 0x5fd7d7, 0x5fd7ff,
        0x5fff00, 0x5fff5f, 0x5fff87, 0x5fffaf, 0x5fffd7, 0x5fffff, 0x870000, 0x87005f, 0x870087, 0x8700af, 0x8700d7,
        0x8700ff, 0x875f00, 0x875f5f, 0x875f87, 0x875faf, 0x875fd7, 0x875fff, 0x878700, 0x87875f, 0x878787, 0x8787af,
        0x8787d7, 0x8787ff, 0x87af00, 0x87af5f, 0x87af87, 0x87afaf, 0x87afd7, 0x87afff, 0x87d700, 0x87d75f, 0x87d787,
        0x87d7af, 0x87d7d7, 0x87d7ff, 0x87ff00, 0x87ff5f, 0x87ff87, 0x87ffaf, 0x87ffd7, 0x87ffff, 0xaf0000, 0xaf005f,
        0xaf0087, 0xaf00af, 0xaf00d7, 0xaf00ff, 0xaf5f00, 0xaf5f5f, 0xaf5f87, 0xaf5faf, 0xaf5fd7, 0xaf5fff, 0xaf8700,
        0xaf875f, 0xaf8787, 0xaf87af, 0xaf87d7, 0xaf87ff, 0xafaf00, 0xafaf5f, 0xafaf87, 0xafafaf, 0xafafd7, 0xafafff,
        0xafd700, 0xafd75f, 0xafd787, 0xafd7af, 0xafd7d7, 0xafd7ff, 0xafff00, 0xafff5f, 0xafff87, 0xafffaf, 0xafffd7,
        0xafffff, 0xd70000, 0xd7005f, 0xd70087, 0xd700af, 0xd700d7, 0xd700ff, 0xd75f00, 0xd75f5f, 0xd75f87, 0xd75faf,
        0xd75fd7, 0xd75fff, 0xd78700, 0xd7875f, 0xd78787, 0xd787af, 0xd787d7, 0xd787ff, 0xd7af00, 0xd7af5f, 0xd7af87,
        0xd7afaf, 0xd7afd7, 0xd7afff, 0xd7d700, 0xd7d75f, 0xd7d787, 0xd7d7af, 0xd7d7d7, 0xd7d7ff, 0xd7ff00, 0xd7ff5f,
        0xd7ff87, 0xd7ffaf, 0xd7ffd7, 0xd7ffff, 0xff0000, 0xff005f, 0xff0087, 0xff00af, 0xff00d7, 0xff00ff, 0xff5f00,
        0xff5f5f, 0xff5f87, 0xff5faf, 0xff5fd7, 0xff5fff, 0xff8700, 0xff875f, 0xff8787, 0xff87af, 0xff87d7, 0xff87ff,
        0xffaf00, 0xffaf5f, 0xffaf87, 0xffafaf, 0xffafd7, 0xffafff, 0xffd700, 0xffd75f, 0xffd787, 0xffd7af, 0xffd7d7,
        0xffd7ff, 0xffff00, 0xffff5f, 0xffff87, 0xffffaf, 0xffffd7, 0xffffff, 0x080808, 0x121212, 0x1c1c1c, 0x262626,
        0x303030, 0x3a3a3a, 0x444444, 0x4e4e4e, 0x585858, 0x626262, 0x6c6c6c, 0x767676, 0x808080, 0x8a8a8a, 0x949494,
        0x9e9e9e, 0xa8a8a8, 0xb2b2b2, 0xbcbcbc, 0xc6c6c6, 0xd0d0d0, 0xdadada, 0xe4e4e4, 0xeeeeee,
    ];
    16 + convert_color(rgb, K_COLORS)
}

/// Down-sample a parsed colour to whatever the terminal can actually render.
fn color_to_vterm_color(c: Color, color_support: ColorSupport) -> VTermColor {
    match c {
        Color::Named(idx) => VTermColor::from_idx(idx),
        Color::Rgb(rgb) if color_support.contains(ColorSupport::TERM24BIT) => {
            VTermColor::from_rgb(rgb[0], rgb[1], rgb[2])
        },
        Color::Rgb(rgb) if color_support.contains(ColorSupport::TERM256) => {
            VTermColor::from_idx(term256_color_for_rgb(rgb))
        },
        Color::Rgb(rgb) => VTermColor::from_idx(term16_color_for_rgb(rgb)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_string())
    }

    #[test]
    fn named_colors_are_sorted() {
        assert!(NAMED_COLORS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn parses_named_colors() {
        for (name, idx) in NAMED_COLORS {
            assert_eq!(try_parse_named(name), Some(Color::Named(*idx)), "{name}");
            assert_eq!(
                try_parse_named(&name.to_ascii_uppercase()),
                Some(Color::Named(*idx)),
                "{name} uppercased"
            );
        }
        // Mixed case, as upstream accepted.
        assert_eq!(try_parse_named("BrBlue"), Some(Color::Named(12)));
        assert_eq!(try_parse_named("bRYelLow"), Some(Color::Named(11)));

        // Not colors, and no abbreviations.
        for name in ["", "aaa", "blu", "other", "bl", "bright", "brblacks"] {
            assert_eq!(try_parse_named(name), None, "{name}");
        }
    }

    #[test]
    fn parses_rgb() {
        assert_eq!(try_parse_rgb("#123"), Some(Color::Rgb([0x11, 0x22, 0x33])));
        assert_eq!(try_parse_rgb("123"), Some(Color::Rgb([0x11, 0x22, 0x33])));
        assert_eq!(try_parse_rgb("#f3a035"), Some(Color::Rgb([0xf3, 0xa0, 0x35])));
        assert_eq!(try_parse_rgb("#F3A035"), Some(Color::Rgb([0xf3, 0xa0, 0x35])));
        assert_eq!(try_parse_rgb("#fff"), Some(Color::Rgb([0xff, 0xff, 0xff])));
        assert_eq!(try_parse_rgb("#000000"), Some(Color::Rgb([0, 0, 0])));
        for spec in ["abc", "abcabc", "ffffff", "000000", "000", "fff"] {
            assert!(try_parse_rgb(spec).is_some(), "{spec}");
        }

        // Wrong length or not hex.
        for spec in ["#xyz", "12", "abcdeh", "#ffff", "12345", "1234567", "", "#", "#12345g"] {
            assert_eq!(try_parse_rgb(spec), None, "{spec}");
        }
    }

    #[test]
    fn downsamples_rgb_to_what_the_terminal_supports() {
        let red = Color::Rgb([0xff, 0x00, 0x00]);
        assert_eq!(
            color_to_vterm_color(red, ColorSupport::TERM24BIT),
            VTermColor::from_rgb(0xff, 0, 0)
        );
        // TERM24BIT wins when both are set.
        assert_eq!(
            color_to_vterm_color(red, ColorSupport::TERM24BIT | ColorSupport::TERM256),
            VTermColor::from_rgb(0xff, 0, 0)
        );
        // 256-color palette: 16 + the index of #ff0000 in the cube.
        assert_eq!(
            color_to_vterm_color(red, ColorSupport::TERM256),
            VTermColor::from_idx(196)
        );
        // 16-color palette: bright red.
        assert_eq!(
            color_to_vterm_color(red, ColorSupport::empty()),
            VTermColor::from_idx(9)
        );
        // Named colors ignore color support entirely.
        for support in [ColorSupport::empty(), ColorSupport::TERM256, ColorSupport::TERM24BIT] {
            assert_eq!(color_to_vterm_color(Color::Named(6), support), VTermColor::from_idx(6));
        }
    }

    #[test]
    fn zsh_named_colors() {
        for (name, idx) in NAMED_COLORS {
            assert_eq!(
                parse_suggestion_color_zsh_autosuggest(&format!("fg={name}"), ColorSupport::TERM256),
                SuggestionColor {
                    fg: Some(VTermColor::from_idx(*idx)),
                    bg: None,
                },
                "fg={name}"
            );
        }
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=cyan,underline", ColorSupport::empty()),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(6)),
                bg: None,
            }
        );
    }

    #[test]
    fn zsh_rgb() {
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=#123,bg=#456", ColorSupport::TERM24BIT),
            SuggestionColor {
                fg: Some(VTermColor::from_rgb(0x11, 0x22, 0x33)),
                bg: Some(VTermColor::from_rgb(0x44, 0x55, 0x66)),
            }
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=#ff0000,bold", ColorSupport::TERM24BIT),
            SuggestionColor {
                fg: Some(VTermColor::from_rgb(0xff, 0, 0)),
                bg: None,
            }
        );
        // Without 24-bit support the color is mapped onto the palette.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=#123,bg=#456", ColorSupport::empty()),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(0)),
                bg: Some(VTermColor::from_idx(8)),
            }
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=#ff0000", ColorSupport::TERM256),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(196)),
                bg: None,
            }
        );
        // The zsh path only tries hex after a '#'; a bare hex string falls through to the index
        // path and, failing that, to index 0.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=ff0000", ColorSupport::TERM24BIT),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(0)),
                bg: None,
            }
        );
    }

    #[test]
    fn zsh_indexed() {
        for idx in 0..=255u16 {
            let parsed = parse_suggestion_color_zsh_autosuggest(&format!("fg={idx}"), ColorSupport::TERM256);
            assert_eq!(parsed.fg, Some(VTermColor::from_idx(idx as u8)), "fg={idx}");
        }
        // With no color support anything from 16 up is rejected and leaves the field untouched.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=15", ColorSupport::empty()).fg,
            Some(VTermColor::from_idx(15))
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=16", ColorSupport::empty()).fg,
            None
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=256", ColorSupport::TERM256).fg,
            None
        );
        // A rejected index does not clear an earlier one.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=8,fg=999", ColorSupport::TERM256).fg,
            Some(VTermColor::from_idx(8))
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=8", ColorSupport::empty()),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(8)),
                bg: None,
            }
        );
    }

    #[test]
    fn zsh_attributes_are_skipped() {
        for attr in [
            "bold",
            "underline",
            "standout",
            "reverse",
            "italic",
            "blink",
            "none",
            "bg=",
        ] {
            let parsed = parse_suggestion_color_zsh_autosuggest(&format!("fg=8,{attr}"), ColorSupport::TERM256);
            assert_eq!(parsed.fg, Some(VTermColor::from_idx(8)), "{attr}");
            if attr != "bg=" {
                assert_eq!(parsed.bg, None, "{attr}");
            }
        }
        // The real-world zsh-autosuggestions default.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=8", ColorSupport::TERM256),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(8)),
                bg: None,
            }
        );
    }

    #[test]
    fn zsh_invalid_input() {
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("", ColorSupport::TERM256),
            SuggestionColor { fg: None, bg: None }
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("bold,underline", ColorSupport::TERM256),
            SuggestionColor { fg: None, bg: None }
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("invalid=!,,=,bg=cyan", ColorSupport::empty()),
            SuggestionColor {
                fg: None,
                bg: Some(VTermColor::from_idx(6)),
            }
        );
        // Unparseable colors fall back to index 0, they are not ignored.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=nonsense", ColorSupport::TERM256).fg,
            Some(VTermColor::from_idx(0))
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=", ColorSupport::TERM256).fg,
            Some(VTermColor::from_idx(0))
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=-1", ColorSupport::TERM256).fg,
            Some(VTermColor::from_idx(0))
        );
        // Not a char boundary at byte 3: skipped, like any segment without an fg=/bg= prefix.
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg\u{20ac}=cyan", ColorSupport::TERM256),
            SuggestionColor { fg: None, bg: None }
        );
    }

    #[test]
    fn zsh_last_segment_wins() {
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("fg=red,fg=blue", ColorSupport::TERM256).fg,
            Some(VTermColor::from_idx(4))
        );
        assert_eq!(
            parse_suggestion_color_zsh_autosuggest("bg=1,bg=2,fg=3", ColorSupport::TERM256),
            SuggestionColor {
                fg: Some(VTermColor::from_idx(3)),
                bg: Some(VTermColor::from_idx(2)),
            }
        );
    }

    #[test]
    fn suggestion_color_accessors() {
        let sc = SuggestionColor {
            fg: Some(VTermColor::from_idx(1)),
            bg: None,
        };
        assert_eq!(sc.fg(), Some(VTermColor::from_idx(1)));
        assert_eq!(sc.bg(), None);
    }

    #[test]
    fn color_support_flags() {
        assert!(ColorSupport::empty().is_empty());
        assert!(!ColorSupport::TERM256.is_empty());
        assert!(ColorSupport::empty().contains(ColorSupport::empty()));
        assert!(!ColorSupport::TERM256.contains(ColorSupport::TERM24BIT));

        let both = ColorSupport::TERM256 | ColorSupport::TERM24BIT;
        assert!(both.contains(ColorSupport::TERM256));
        assert!(both.contains(ColorSupport::TERM24BIT));

        let mut acc = ColorSupport::empty();
        acc |= ColorSupport::TERM24BIT;
        assert_eq!(acc, ColorSupport::TERM24BIT);

        assert_eq!(ColorSupport::default(), ColorSupport::empty());
    }

    #[test]
    fn color_support_debug_matches_bitflags() {
        assert_eq!(format!("{:?}", ColorSupport::empty()), "ColorSupport(0x0)");
        assert_eq!(format!("{:?}", ColorSupport::TERM256), "ColorSupport(TERM256)");
        assert_eq!(format!("{:?}", ColorSupport::TERM24BIT), "ColorSupport(TERM24BIT)");
        assert_eq!(
            format!("{:?}", ColorSupport::TERM256 | ColorSupport::TERM24BIT),
            "ColorSupport(TERM256 | TERM24BIT)"
        );
    }

    #[test]
    fn bools_from_strings() {
        for yes in ["y", "Y", "t", "T", "1", "yes", "true", "1000"] {
            assert!(bool_from_string(yes), "{yes}");
        }
        for no in ["", "n", "N", "f", "0", "no", "false", "off"] {
            assert!(!bool_from_string(no), "{no}");
        }
    }

    #[test]
    fn color_support_from_environment() {
        let assert_supports = |vars: &[(&str, &str)], expected: ColorSupport| {
            assert_eq!(color_support_from_env(env_from(vars)), expected, "{vars:?}");
        };

        assert_supports(&[], ColorSupport::empty());

        // $fish_term256 wins outright.
        assert_supports(&[("fish_term256", "y")], ColorSupport::TERM256);
        assert_supports(&[("fish_term256", "n")], ColorSupport::empty());
        assert_supports(
            &[("fish_term256", "n"), ("TERM", "xterm-256color")],
            ColorSupport::empty(),
        );
        // ... otherwise $TERM decides.
        assert_supports(&[("TERM", "foo_256color_bar")], ColorSupport::TERM256);
        assert_supports(&[("TERM", "xterm")], ColorSupport::TERM256);
        assert_supports(&[("TERM", "dumb")], ColorSupport::empty());
        // Terminal.app only from Snow Leopard on.
        assert_supports(
            &[
                ("TERM", "xterm"),
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM_PROGRAM_VERSION", "300"),
            ],
            ColorSupport::TERM256,
        );
        assert_supports(
            &[
                ("TERM", "xterm"),
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM_PROGRAM_VERSION", "200"),
            ],
            ColorSupport::empty(),
        );
        assert_supports(
            &[
                ("TERM", "xterm"),
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM_PROGRAM_VERSION", "not a number"),
            ],
            ColorSupport::empty(),
        );

        // $fish_term24bit wins outright.
        assert_supports(&[("fish_term24bit", "y")], ColorSupport::TERM24BIT);
        assert_supports(&[("fish_term24bit", "n")], ColorSupport::empty());
        assert_supports(
            &[("fish_term24bit", "n"), ("COLORTERM", "truecolor")],
            ColorSupport::empty(),
        );
        // screen and emacs' ansi-term swallow truecolor sequences.
        assert_supports(&[("TERM", "eterm"), ("STY", "foo")], ColorSupport::empty());
        assert_supports(&[("STY", "foo"), ("COLORTERM", "truecolor")], ColorSupport::empty());
        // $COLORTERM, if set at all, is the end of the chain.
        assert_supports(&[("COLORTERM", "truecolor")], ColorSupport::TERM24BIT);
        assert_supports(&[("COLORTERM", "24bit")], ColorSupport::TERM24BIT);
        assert_supports(&[("COLORTERM", "foo")], ColorSupport::empty());
        assert_supports(&[("COLORTERM", "foo"), ("KONSOLE_VERSION", "1")], ColorSupport::empty());
        assert_supports(&[("KONSOLE_VERSION", "foo")], ColorSupport::TERM24BIT);
        assert_supports(&[("KONSOLE_PROFILE_NAME", "foo")], ColorSupport::TERM24BIT);
        // iTerm session ids contain a colon.
        assert_supports(&[("ITERM_SESSION_ID", "1:2")], ColorSupport::TERM24BIT);
        assert_supports(&[("ITERM_SESSION_ID", "12")], ColorSupport::empty());
        assert_supports(&[("TERM", "st-foo")], ColorSupport::TERM24BIT);
        assert_supports(&[("VTE_VERSION", "3500")], ColorSupport::empty());
        assert_supports(&[("VTE_VERSION", "3700")], ColorSupport::TERM24BIT);
        assert_supports(&[("VTE_VERSION", "not a number")], ColorSupport::empty());

        // Both at once.
        assert_supports(
            &[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")],
            ColorSupport::TERM256 | ColorSupport::TERM24BIT,
        );
    }

    #[test]
    fn get_color_support_does_not_panic() {
        get_color_support();
    }
}
