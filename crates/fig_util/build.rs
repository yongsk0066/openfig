use std::path::Path;

const BRANDING_ENV: &str = "../../branding.env";

/// Re-exports every key of the repo root `branding.env` as a compile time
/// `FIG_BRAND_<KEY>` env var so that `consts.rs` can read them with `env!`.
///
/// The Justfile reads the same file through `set dotenv-filename`, so this
/// parser has to agree with just's dotenv parser on every shape it accepts.
/// Where the two disagree the bundle directory name and `APP_BUNDLE_NAME` drift
/// apart, the app silently falls back to `/Applications/<other name>.app` and
/// every resource 404s with no log line (`06-branding.md` §2.8). Anything this
/// parser cannot read exactly the way just does is therefore a hard error, not
/// a guess.
///
/// Accepted (verified against just 1.58.0):
///
/// ```text
/// # comment                whole-line comment, blank lines
/// KEY=value                unquoted; may not contain whitespace
/// KEY=value  # comment     a `#` after whitespace ends an unquoted value
/// KEY="value with spaces"  double quoted; no backslash escapes
/// KEY='value with spaces'  single quoted; literal
/// export KEY=value         optional `export` prefix
/// KEY = value              optional spaces around `=`
/// ```
fn main() {
    self_check();

    println!("cargo:rerun-if-changed={BRANDING_ENV}");

    let path = Path::new(BRANDING_ENV);
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));

    for line in contents.lines() {
        if let Some((key, value)) = parse_line(line) {
            println!("cargo:rustc-env=FIG_BRAND_{key}={value}");
        }
    }
}

/// `None` for blank lines and whole-line comments.
fn parse_line(line: &str) -> Option<(&str, &str)> {
    // `trim` also drops the `\r` of a CRLF file.
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let unprefixed = match line.strip_prefix("export") {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => line,
    };

    let (key, rest) = unprefixed
        .split_once('=')
        .unwrap_or_else(|| panic!("{BRANDING_ENV}: line has no '=': {line}"));

    let key = key.trim();
    if key.is_empty()
        || !key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        panic!("{BRANDING_ENV}: invalid key {key:?}: {line}");
    }

    let rest = rest.trim_start();
    let value = match rest.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let body = &rest[quote.len_utf8()..];
            let end = body
                .find(quote)
                .unwrap_or_else(|| panic!("{BRANDING_ENV}: unterminated {quote} quote: {line}"));
            let (value, tail) = body.split_at(end);

            // just resolves `\"`, `\n` and friends; rather than reimplement that
            // and risk a mismatch, refuse a value that would need it.
            if quote == '"' && value.contains('\\') {
                panic!("{BRANDING_ENV}: backslash escapes are not supported, keep the value literal: {line}");
            }

            let tail = tail[quote.len_utf8()..].trim_start();
            if !tail.is_empty() && !tail.starts_with('#') {
                panic!("{BRANDING_ENV}: trailing text after the closing {quote} quote: {line}");
            }

            value
        },
        _ => {
            let value = match comment_start(rest) {
                Some(hash) => rest[..hash].trim_end(),
                None => rest.trim_end(),
            };

            if value.contains(['"', '\'']) {
                panic!("{BRANDING_ENV}: unmatched quote: {line}");
            }
            if value.contains(char::is_whitespace) {
                panic!("{BRANDING_ENV}: unquoted value contains whitespace, quote it: {line}");
            }

            value
        },
    };

    Some((key, value))
}

/// Offset of the `#` that starts a trailing comment. just only treats a `#` as a
/// comment when it opens the value or follows whitespace, so `KEY=a#b` is the
/// three character value `a#b`.
fn comment_start(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    (0..bytes.len()).find(|&i| bytes[i] == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()))
}

/// Runs on every build. Every case here was measured against just's dotenv
/// parser; the first four are the ones the original parser got wrong.
fn self_check() {
    let cases: &[(&str, Option<(&str, &str)>)] = &[
        ("APP_NAME='Fig'", Some(("APP_NAME", "Fig"))),
        ("APP_NAME=Fig  # brand", Some(("APP_NAME", "Fig"))),
        ("APP_NAME=\"Fig\" # brand", Some(("APP_NAME", "Fig"))),
        ("export APP_NAME=Fig", Some(("APP_NAME", "Fig"))),
        ("APP_NAME=Fig", Some(("APP_NAME", "Fig"))),
        ("APP_NAME=\"Fig Dev\"", Some(("APP_NAME", "Fig Dev"))),
        ("APP_NAME='Fig Dev'", Some(("APP_NAME", "Fig Dev"))),
        ("APP_NAME = Fig", Some(("APP_NAME", "Fig"))),
        ("APP_NAME=  Fig  ", Some(("APP_NAME", "Fig"))),
        (
            "BACKUP_DIR_NAME=.fig.dotfiles.bak",
            Some(("BACKUP_DIR_NAME", ".fig.dotfiles.bak")),
        ),
        ("APP_NAME=a#b", Some(("APP_NAME", "a#b"))),
        ("APP_NAME=", Some(("APP_NAME", ""))),
        ("  # comment", None),
        ("", None),
    ];

    for &(line, expected) in cases {
        assert_eq!(
            parse_line(line),
            expected,
            "branding.env parser disagrees with just on {line:?}"
        );
    }
}
