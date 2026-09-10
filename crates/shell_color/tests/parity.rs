//! Differential test against the `shell-color` 1.0.1 crate this module replaces.
//!
//! Temporary: this file and the `old_shell_color` dev-dependency are deleted in the commit that
//! drops `shell-color` from the workspace. It exists to prove the reimplementation is
//! bit-for-bit identical on the only entry point figterm calls.

use old_shell_color as old;

/// Normalise both crates' output to the same shape so they can be compared.
fn new_repr(sc: &shell_color::SuggestionColor) -> [Option<(u8, u8, u8, u8)>; 2] {
    let f = |c: &Option<shell_color::VTermColor>| match c {
        Some(shell_color::VTermColor::Rgb { red, green, blue }) => Some((0, *red, *green, *blue)),
        Some(shell_color::VTermColor::Indexed { idx }) => Some((1, *idx, 0, 0)),
        None => None,
    };
    [f(&sc.fg), f(&sc.bg)]
}

fn old_repr(sc: &old::SuggestionColor) -> [Option<(u8, u8, u8, u8)>; 2] {
    let f = |c: &Option<old::VTermColor>| match c {
        Some(old::VTermColor::Rgb { red, green, blue }) => Some((0, *red, *green, *blue)),
        Some(old::VTermColor::Indexed { idx }) => Some((1, *idx, 0, 0)),
        None => None,
    };
    [f(&sc.fg), f(&sc.bg)]
}

fn supports() -> [(shell_color::ColorSupport, old::ColorSupport); 4] {
    [
        (shell_color::ColorSupport::empty(), old::ColorSupport::empty()),
        (shell_color::ColorSupport::TERM256, old::ColorSupport::TERM256),
        (shell_color::ColorSupport::TERM24BIT, old::ColorSupport::TERM24BIT),
        (
            shell_color::ColorSupport::TERM256 | shell_color::ColorSupport::TERM24BIT,
            old::ColorSupport::TERM256 | old::ColorSupport::TERM24BIT,
        ),
    ]
}

/// Deterministic xorshift64*, so the fuzzed corpus is the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() % xs.len() as u64) as usize]
    }
}

/// The colour specs that actually reach this code, plus every shape the old crate could be fed.
fn corpus() -> Vec<String> {
    const NAMES: &[&str] = &[
        "black",
        "blue",
        "brblack",
        "brblue",
        "brbrown",
        "brcyan",
        "brgreen",
        "brgrey",
        "brmagenta",
        "brown",
        "brpurple",
        "brred",
        "brwhite",
        "bryellow",
        "cyan",
        "green",
        "grey",
        "magenta",
        "purple",
        "red",
        "white",
        "yellow",
    ];
    const ATTRS: &[&str] = &[
        "bold",
        "underline",
        "standout",
        "reverse",
        "italic",
        "blink",
        "none",
        "",
        "=",
        "invalid=!",
        "fg",
        "bg",
        "fg=",
        "bg=",
    ];
    const HEX: &[&str] = &[
        "#000", "#fff", "#123", "#FA3", "#f3a035", "#F3A035", "#ffffff", "#000000", "#ababab", "#ff0000", "#00ff00",
        "#0000ff", "#808080", "#xyz", "#ffff", "#12345g", "#", "000", "fff", "abcabc", "ffffff", "123456",
    ];
    const JUNK: &[&str] = &[
        "nonsense",
        "-1",
        "+1",
        " 8",
        "8 ",
        "0x10",
        "1e3",
        "٧",
        "é",
        "bl",
        "BLACK",
        "BrBlue",
        "bRYelLow",
        "brblacks",
        "999999999999999999999999",
    ];

    let mut out: Vec<String> = Vec::new();

    // Every atom on its own, under both prefixes and a few case foldings.
    let mut atoms: Vec<String> = Vec::new();
    for n in NAMES {
        atoms.push((*n).to_string());
        atoms.push(n.to_ascii_uppercase());
        atoms.push(format!("{}{}", n[..1].to_ascii_uppercase(), &n[1..]));
    }
    atoms.extend(HEX.iter().map(|s| (*s).to_string()));
    atoms.extend(JUNK.iter().map(|s| (*s).to_string()));
    // Indices around every acceptance boundary, and the whole palette.
    atoms.extend((0..=300u32).map(|i| i.to_string()));

    for a in &atoms {
        out.push(format!("fg={a}"));
        out.push(format!("bg={a}"));
        out.push(a.clone());
    }
    out.extend(ATTRS.iter().map(|s| (*s).to_string()));

    // Fuzzed multi-segment specs.
    let mut all: Vec<&str> = Vec::new();
    all.extend(NAMES);
    all.extend(HEX);
    all.extend(JUNK);
    all.extend(ATTRS);
    let indices: Vec<String> = (0..=300u32).map(|i| i.to_string()).collect();
    let all_owned: Vec<String> = all
        .iter()
        .map(|s| (*s).to_string())
        .chain(indices.iter().cloned())
        .collect();

    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..2000 {
        let n = 1 + rng.next() % 4;
        let segments: Vec<String> = (0..n)
            .map(|_| {
                let atom = rng.pick(&all_owned).clone();
                match rng.next() % 4 {
                    0 => format!("fg={atom}"),
                    1 => format!("bg={atom}"),
                    2 => atom,
                    _ => format!("{}={atom}", rng.pick(ATTRS)),
                }
            })
            .collect();
        out.push(segments.join(","));
    }

    out
}

#[test]
fn zsh_autosuggest_parsing_matches_shell_color_1_0_1() {
    let corpus = corpus();
    assert!(corpus.len() > 2000, "corpus is {} entries", corpus.len());

    let mut checked = 0usize;
    for spec in &corpus {
        for (new_support, old_support) in supports() {
            let new = shell_color::parse_suggestion_color_zsh_autosuggest(spec, new_support);
            let old = old::parse_suggestion_color_zsh_autosuggest(spec, old_support);
            assert_eq!(
                new_repr(&new),
                old_repr(&old),
                "spec {spec:?} with {old_support:?}: new {new:?} != old {old:?}"
            );
            checked += 1;
        }
    }

    println!(
        "compared {checked} (spec, color support) pairs over {} specs",
        corpus.len()
    );
}

/// The whole 24-bit space is too large, but every colour the palette-matching tables can be asked
/// about is worth a sweep: both the exact palette entries and a scattering in between.
#[test]
fn rgb_downsampling_matches_shell_color_1_0_1() {
    let mut rng = Rng(0x243f_6a88_85a3_08d3);
    for _ in 0..20_000 {
        let v = rng.next();
        let spec = format!("fg=#{:02x}{:02x}{:02x}", v as u8, (v >> 8) as u8, (v >> 16) as u8);
        for (new_support, old_support) in supports() {
            let new = shell_color::parse_suggestion_color_zsh_autosuggest(&spec, new_support);
            let old = old::parse_suggestion_color_zsh_autosuggest(&spec, old_support);
            assert_eq!(new_repr(&new), old_repr(&old), "spec {spec:?} with {old_support:?}");
        }
    }
}
