//! The line protocol between `fig.zsh` and the desktop.
//!
//! zsh has no protobuf, so the hook speaks lines of text: a verb, then fields separated by single
//! spaces, then `\n`. Every string field is escaped with [`escape`], which turns `\`, LF, CR, TAB
//! and space into two-character backslash sequences, so no field ever contains a space or a line
//! break and every line splits the same way on both sides. The zsh half of each function here is
//! in `crates/fig_integrations/src/shell/scripts/fig.zsh`.
//!
//! Shell → desktop:
//!
//! ```text
//! HELLO <version> <session> <shell> <pid> <tty> <user@host>   first line, once
//! PING                         instead of HELLO: is anyone there (`doctor`); no session
//! C <env> <alias>              exported variables as NUL-separated k=v; `alias` as k=v lines
//! P <exit status> <cwd>        a prompt is about to be drawn (precmd)
//! E <cursor chars> <cursor bytes> <text>   $PREBUFFER$BUFFER and the cursor in it
//! X <command>                  a command line was accepted (preexec)
//! K <key>                      a bound key the desktop asked to see (`INTERCEPT 1`)
//! ```
//!
//! Desktop → shell:
//!
//! ```text
//! PONG <version>               the answer to PING, then the desktop closes
//! WELCOME 1 <version>          HELLO accepted
//! WELCOME 0 <reason>           refused; the desktop closes and the shell stops asking
//! INSERT <delete before> <text> <cursor left> <accept>
//! INTERCEPT 0 | INTERCEPT 1 <key>...
//! KEY 0|1|2 <key>              the answer to `K`: zsh keeps the key (0), the desktop took it and may
//!                              answer with an insertion (1), or took it and nothing follows (2)
//! BYE                          stop for the rest of this shell's life
//! ```
//!
//! Counts in `INSERT` are characters, as `LBUFFER` and `CURSOR` count them. `accept` 1 runs the
//! line after the edit — but only where zsh can: inside the key widget that is waiting after `KEY
//! 1` (for at most 80 ms). An `INSERT` that arrives later is applied by the `zle -F` handler, and
//! zsh (5.9, `raw_getbyte`) goes back to waiting for a key after a handler without looking at what
//! it asked for, so `accept-line` there would take effect only after the next key — with that key
//! typed into the command. Such a line is typed and left for the user to run.

use std::collections::BTreeSet;
use std::fmt;

pub use fig_util::HOOK_PROTOCOL_VERSION;

/// Longest line either side accepts, newline excluded. The edit buffer is the only unbounded field,
/// and a buffer this size is a paste nobody wants suggestions for.
pub const MAX_LINE_LEN: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    UnknownVerb(String),
    MissingField(&'static str),
    BadNumber(&'static str, String),
    /// A backslash followed by something [`escape`] never writes, or by nothing.
    BadEscape(Option<char>),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::UnknownVerb(verb) => write!(f, "unknown verb {verb:?}"),
            WireError::MissingField(field) => write!(f, "missing field {field}"),
            WireError::BadNumber(field, value) => write!(f, "{field} is not a number: {value:?}"),
            WireError::BadEscape(Some(c)) => write!(f, "unknown escape \\{c}"),
            WireError::BadEscape(None) => write!(f, "trailing backslash"),
        }
    }
}

impl std::error::Error for WireError {}

/// One string as one space-free field.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' ' => out.push_str("\\s"),
            c => out.push(c),
        }
    }
    out
}

/// The exact inverse of [`escape`]. Anything `escape` cannot have produced is an error rather than
/// a guess.
pub fn unescape(s: &str) -> Result<String, WireError> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next() {
            Some('\\') => '\\',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('s') => ' ',
            other => return Err(WireError::BadEscape(other)),
        });
    }
    Ok(out)
}

/// A line from the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellLine {
    Hello {
        version: u32,
        session: String,
        shell: String,
        pid: Option<i32>,
        tty: String,
        user: String,
    },
    Context {
        env: Vec<(String, String)>,
        alias: String,
    },
    Prompt {
        exit_code: i32,
        cwd: String,
    },
    EditBuffer {
        text: String,
        /// UTF-8 byte offset of the cursor in `text`, what `EditBufferHook.cursor` carries.
        cursor: usize,
    },
    PreExec {
        command: String,
    },
    Key {
        name: String,
    },
}

struct Fields<'a>(std::str::Split<'a, char>);

impl<'a> Fields<'a> {
    fn raw(&mut self, name: &'static str) -> Result<&'a str, WireError> {
        self.0.next().ok_or(WireError::MissingField(name))
    }

    fn number<T: std::str::FromStr>(&mut self, name: &'static str) -> Result<T, WireError> {
        let raw = self.raw(name)?;
        raw.parse().map_err(|_err| WireError::BadNumber(name, raw.into()))
    }

    /// A string field; a missing trailing one is the empty string, which zsh writes as nothing.
    fn text(&mut self) -> Result<String, WireError> {
        unescape(self.0.next().unwrap_or_default())
    }
}

impl ShellLine {
    pub fn parse(line: &str) -> Result<Self, WireError> {
        let mut fields = Fields(line.split(' '));
        let verb = fields.raw("verb")?;
        Ok(match verb {
            // Only the version has to parse: a newer script may say anything after it, and it is
            // owed a `WELCOME 0` rather than a dropped connection.
            "HELLO" => ShellLine::Hello {
                version: fields.number("version")?,
                session: fields.text().unwrap_or_default(),
                shell: fields.text().unwrap_or_default(),
                pid: fields.number("pid").ok(),
                tty: fields.text().unwrap_or_default(),
                user: fields.text().unwrap_or_default(),
            },
            "C" => ShellLine::Context {
                env: fields
                    .text()?
                    .split('\0')
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(key, value)| (key.to_owned(), value.to_owned()))
                    .collect(),
                alias: fields.text()?,
            },
            "P" => ShellLine::Prompt {
                exit_code: fields.number("exit status")?,
                cwd: fields.text()?,
            },
            "E" => {
                let chars: usize = fields.number("cursor chars")?;
                let bytes: usize = fields.number("cursor bytes")?;
                let text = fields.text()?;
                let cursor = cursor_offset(&text, chars, bytes);
                ShellLine::EditBuffer { text, cursor }
            },
            "X" => ShellLine::PreExec {
                command: fields.text()?,
            },
            "K" => ShellLine::Key { name: fields.text()? },
            verb => return Err(WireError::UnknownVerb(verb.into())),
        })
    }
}

/// The cursor as a byte offset into `text`.
///
/// zsh counts both: characters in its locale (`$CURSOR`) and bytes (`${#LBUFFER}` without
/// `multibyte`). The byte count is right in any locale as long as it lands between characters of
/// the decoded text; a shell in the C locale counts characters as bytes, so the character count is
/// only the fallback. Past the end means the end.
fn cursor_offset(text: &str, chars: usize, bytes: usize) -> usize {
    if bytes <= text.len() && text.is_char_boundary(bytes) {
        bytes
    } else {
        text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
    }
}

/// An edit `fig.zsh` applies to `LBUFFER`, in characters: delete before the cursor, insert, move
/// the cursor left (negative: right), then run the line if `accept`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Insert {
    pub delete_before: usize,
    pub text: String,
    pub cursor_left: isize,
    pub accept: bool,
}

impl Insert {
    pub fn is_noop(&self) -> bool {
        self.delete_before == 0 && self.text.is_empty() && self.cursor_left == 0 && !self.accept
    }
}

/// The answer to a `K`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyVerdict {
    /// Interception ended after the shell last heard: zsh acts on the key after all.
    Pass      = 0,
    /// Taken, and the action may insert: the shell waits briefly so a line the insertion ends with
    /// can be run.
    MayInsert = 1,
    /// Taken, nothing will follow.
    Taken     = 2,
}

pub use fig_util::HOOK_PING as PING;

/// A line to the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonLine {
    Pong,
    Welcome,
    Refuse {
        reason: String,
    },
    Insert(Insert),
    /// The keys `K` should be sent for; empty stops interception.
    Intercept(BTreeSet<String>),
    Key {
        name: String,
        verdict: KeyVerdict,
    },
    Bye,
}

impl fmt::Display for DaemonLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DaemonLine::Pong => write!(f, "PONG {HOOK_PROTOCOL_VERSION}"),
            DaemonLine::Welcome => write!(f, "WELCOME 1 {HOOK_PROTOCOL_VERSION}"),
            DaemonLine::Refuse { reason } => write!(f, "WELCOME 0 {}", escape(reason)),
            DaemonLine::Insert(Insert {
                delete_before,
                text,
                cursor_left,
                accept,
            }) => write!(
                f,
                "INSERT {delete_before} {} {cursor_left} {}",
                escape(text),
                u8::from(*accept)
            ),
            DaemonLine::Intercept(keys) if keys.is_empty() => write!(f, "INTERCEPT 0"),
            DaemonLine::Intercept(keys) => {
                write!(f, "INTERCEPT 1")?;
                keys.iter().try_for_each(|key| write!(f, " {}", escape(key)))
            },
            DaemonLine::Key { name, verdict } => write!(f, "KEY {} {}", *verdict as u8, escape(name)),
            DaemonLine::Bye => write!(f, "BYE"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_round_trips() {
        for s in [
            "",
            " ",
            "git status --short",
            "echo \u{1F600} caf\u{e9}",
            "line one\nline two",
            "crlf\r\n",
            "tab\there",
            "back\\slash",
            "\\s is not a space",
            "\\\\n",
            "trailing\\",
            "nul\0byte",
        ] {
            let escaped = escape(s);
            assert!(!escaped.contains([' ', '\n', '\r', '\t']), "{escaped:?}");
            assert_eq!(unescape(&escaped).unwrap(), s);
        }
    }

    /// The escapes zsh's `_fig_esc` writes for the same inputs, byte for byte.
    #[test]
    fn escape_matches_the_shell() {
        assert_eq!(escape("a b"), "a\\sb");
        assert_eq!(escape("x\ny"), "x\\ny");
        assert_eq!(escape("cr\r\nlf\t"), "cr\\r\\nlf\\t");
        assert_eq!(escape("back\\slash"), "back\\\\slash");
        assert_eq!(escape("\u{e9} \u{1F600}"), "\u{e9}\\s\u{1F600}");
    }

    #[test]
    fn unescape_rejects_what_escape_never_writes() {
        assert_eq!(unescape("a\\"), Err(WireError::BadEscape(None)));
        assert_eq!(unescape("a\\x"), Err(WireError::BadEscape(Some('x'))));
    }

    #[test]
    fn edit_buffer_cursor_is_a_byte_offset() {
        let parse = |line| ShellLine::parse(line).unwrap();
        assert_eq!(parse("E 0 0 "), ShellLine::EditBuffer {
            text: "".into(),
            cursor: 0
        });
        assert_eq!(parse("E 0 0"), ShellLine::EditBuffer {
            text: "".into(),
            cursor: 0
        });
        assert_eq!(parse("E 6 6 git\\sst"), ShellLine::EditBuffer {
            text: "git st".into(),
            cursor: 6
        });
        // "echo 😀 ok" with the cursor after the emoji: 6 characters, 9 bytes.
        assert_eq!(parse("E 6 9 echo\\s\u{1F600}\\sok"), ShellLine::EditBuffer {
            text: "echo \u{1F600} ok".into(),
            cursor: 9
        });
        // A C-locale shell counts bytes as characters; the byte count still lands on a boundary.
        assert_eq!(parse("E 9 9 echo\\s\u{1F600}\\sok"), ShellLine::EditBuffer {
            text: "echo \u{1F600} ok".into(),
            cursor: 9
        });
        // Inside a character, or past the end: fall back to the character count, then the end.
        assert_eq!(parse("E 6 7 echo\\s\u{1F600}\\sok"), ShellLine::EditBuffer {
            text: "echo \u{1F600} ok".into(),
            cursor: 9
        });
        assert_eq!(parse("E 99 99 ab"), ShellLine::EditBuffer {
            text: "ab".into(),
            cursor: 2
        });
        // PS2: $PREBUFFER ends in a newline.
        assert_eq!(parse("E 10 10 echo\\s\"abc\\n"), ShellLine::EditBuffer {
            text: "echo \"abc\n".into(),
            cursor: 10
        });
    }

    #[test]
    fn shell_lines_parse() {
        assert_eq!(
            ShellLine::parse("HELLO 1 123.456 zsh 123 /dev/ttys001 me@host").unwrap(),
            ShellLine::Hello {
                version: 1,
                session: "123.456".into(),
                shell: "zsh".into(),
                pid: Some(123),
                tty: "/dev/ttys001".into(),
                user: "me@host".into(),
            }
        );
        // A future script: the version is enough to refuse it properly.
        assert!(matches!(
            ShellLine::parse("HELLO 7 whatever\\q"),
            Ok(ShellLine::Hello { version: 7, .. })
        ));
        assert_eq!(
            ShellLine::parse("C PATH=/bin:/usr/bin\0HOME=/Users/me\0EMPTY=\0A=b=c ll=ls\\s-l\\ngs=git\\sstatus")
                .unwrap(),
            ShellLine::Context {
                env: vec![
                    ("PATH".into(), "/bin:/usr/bin".into()),
                    ("HOME".into(), "/Users/me".into()),
                    ("EMPTY".into(), "".into()),
                    ("A".into(), "b=c".into()),
                ],
                alias: "ll=ls -l\ngs=git status".into(),
            }
        );
        assert_eq!(
            ShellLine::parse("P 130 /Users/me/My\\sDocs").unwrap(),
            ShellLine::Prompt {
                exit_code: 130,
                cwd: "/Users/me/My Docs".into()
            }
        );
        assert_eq!(ShellLine::parse("X ls\\s-la").unwrap(), ShellLine::PreExec {
            command: "ls -la".into()
        });
        assert_eq!(ShellLine::parse("K shift+tab").unwrap(), ShellLine::Key {
            name: "shift+tab".into()
        });
        assert_eq!(ShellLine::parse("Z 1"), Err(WireError::UnknownVerb("Z".into())));
        assert_eq!(
            ShellLine::parse("P x /"),
            Err(WireError::BadNumber("exit status", "x".into()))
        );
        assert_eq!(ShellLine::parse("E 1"), Err(WireError::MissingField("cursor bytes")));
    }

    #[test]
    fn daemon_lines_format() {
        assert_eq!(
            DaemonLine::Welcome.to_string(),
            format!("WELCOME 1 {HOOK_PROTOCOL_VERSION}")
        );
        assert_eq!(DaemonLine::Pong.to_string(), format!("PONG {HOOK_PROTOCOL_VERSION}"));
        assert_eq!(
            DaemonLine::Refuse {
                reason: "protocol version 2".into()
            }
            .to_string(),
            "WELCOME 0 protocol\\sversion\\s2"
        );
        assert_eq!(
            DaemonLine::Insert(Insert {
                delete_before: 2,
                text: "one\ntwo \u{1F600}".into(),
                cursor_left: -1,
                accept: true,
            })
            .to_string(),
            "INSERT 2 one\\ntwo\\s\u{1F600} -1 1"
        );
        // An empty text is an empty field: the line keeps its five fields.
        assert_eq!(
            DaemonLine::Insert(Insert {
                cursor_left: 3,
                ..Default::default()
            })
            .to_string(),
            "INSERT 0  3 0"
        );
        assert_eq!(DaemonLine::Intercept(BTreeSet::new()).to_string(), "INTERCEPT 0");
        assert_eq!(
            DaemonLine::Intercept(["tab".to_owned(), "down".to_owned()].into()).to_string(),
            "INTERCEPT 1 down tab"
        );
        assert_eq!(
            DaemonLine::Key {
                name: "enter".into(),
                verdict: KeyVerdict::MayInsert
            }
            .to_string(),
            "KEY 1 enter"
        );
        assert_eq!(
            DaemonLine::Key {
                name: "shift+tab".into(),
                verdict: KeyVerdict::Pass
            }
            .to_string(),
            "KEY 0 shift+tab"
        );
        assert_eq!(
            DaemonLine::Key {
                name: "down".into(),
                verdict: KeyVerdict::Taken
            }
            .to_string(),
            "KEY 2 down"
        );
        assert_eq!(DaemonLine::Bye.to_string(), "BYE");
    }
}
