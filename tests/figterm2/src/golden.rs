//! Edit-buffer golden recordings (docs/15-architecture-study.md §4 stage 2).
//!
//! Each [`Case`] drives a real `/bin/zsh -i` in a pty with a throwaway `ZDOTDIR` that loads this
//! build's shell integration, and records what the pty binary would read from its pty master: the
//! raw output bytes with their read boundaries, and the input and resizes that produced them. The
//! recordings live next to the code that derives edit buffers from them, in upstream alacritty's
//! `tests/ref/<case>/` shape:
//!
//! ```text
//! crates/figterm/tests/ref/<case>/
//!   alacritty.recording   raw pty output, every chunk concatenated
//!   size.json             initial {"columns","screen_lines"}, as upstream
//!   config.json           {"history_size","session_id"} the pty binary runs its `Term` with
//!   events.json           chunk lengths, inputs and resizes in the order they happened, with µs
//!   edit_buffer.json      the expected edit buffer after every input
//! ```
//!
//! `edit_buffer.json` is not what the pty binary happened to derive. It is zsh's own view: a
//! `line-pre-redraw` hook appends `$PREBUFFER$BUFFER` and `$CURSOR` to a side file (never to the
//! pty, so the recording is unaffected) and the last record before the next input becomes that
//! step's expectation, with the cursor converted from characters to the UTF-8 byte offset
//! `EditBufferHook.cursor` carries. `crates/figterm/src/golden.rs` replays the bytes through the
//! pty binary's own parser and edit-buffer code and compares; a case where the two disagree is a
//! defect in the pty binary's screen scraping, not in the recording.
//!
//! Regenerate with `just golden-record` (all cases) or `just golden-record fig_plain,fig_ps2`.

use std::io::{
    Read as _,
    Write as _,
};
use std::path::{
    Path,
    PathBuf,
};
use std::sync::mpsc;
use std::time::{
    Duration,
    Instant,
};

use anyhow::{
    Context as _,
    Result,
    bail,
};
use portable_pty::{
    CommandBuilder,
    PtySize,
    native_pty_system,
};
use serde_json::{
    Value,
    json,
};

/// The shell every case runs. The system one, so a Homebrew zsh earlier on `PATH` cannot change
/// the recordings.
pub const ZSH: &str = "/bin/zsh";

/// `TERM` for every case.
pub const TERM: &str = "xterm-256color";

/// `FIGTERM_SESSION_ID` of the recorded shell. The integration prints it in `NewCmd`, and `Term`
/// ignores a `NewCmd` for any other session, so the replay has to construct its `Term` with it.
pub const SESSION_ID: &str = "golden";

/// `max_scroll_limit` the pty binary passes to `Term::new` (`crates/figterm/src/main.rs`).
pub const HISTORY_SIZE: usize = 1;

/// Size of one read from the pty master, the pty binary's `BUFFER_SIZE`.
const READ_SIZE: usize = 16384;

/// No output for this long after an input means zsh is done with it.
const QUIET: Duration = Duration::from_millis(120);

/// Upper bound for a single input to settle, and for the first prompt.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// One thing done to the shell. Every write is its own step in `events.json` and
/// `edit_buffer.json`.
#[derive(Debug, Clone, Copy)]
pub enum Step {
    /// One write per character, the way a person types.
    Type(&'static str),
    /// All of it in a single write.
    Write(&'static str),
    /// `ESC [200~ … ESC [201~` in a single write, the way a terminal delivers a bracketed paste.
    Paste(&'static str),
    /// `TIOCSWINSZ` on the pty master, which is what the pty binary does on `SIGWINCH`.
    Resize { columns: u16, screen_lines: u16 },
}

#[derive(Debug, Clone, Copy)]
pub struct Case {
    pub name: &'static str,
    pub columns: u16,
    pub screen_lines: u16,
    /// zsh run after [`PRELUDE`], before the integration's `post` half.
    pub zshrc: &'static str,
    pub steps: &'static [Step],
}

/// Common to every case: emacs keys regardless of `EDITOR`, a prompt with no user, host, path,
/// date or git in it.
const PRELUDE: &str = r#"bindkey -e
unsetopt beep
PS1='%# '
"#;

/// Appends zsh's own edit buffer to `$GOLDEN_ZLE_LOG` when a line starts and before every redraw:
/// `<#PREBUFFER> TAB <CURSOR> TAB <PREBUFFER><BUFFER> NUL`, lengths and cursor in characters.
/// `line-pre-redraw` alone misses the empty line of a fresh prompt: zsh does not run it for the
/// first draw.
const ZLE_LOG: &str = r#"if [[ -n ${GOLDEN_ZLE_LOG:-} ]]; then
  autoload -Uz add-zle-hook-widget
  _golden_zle_log() {
    print -rn -- "${#PREBUFFER}"$'\t'"${CURSOR}"$'\t'"${PREBUFFER}${BUFFER}"$'\0' >> $GOLDEN_ZLE_LOG
  }
  add-zle-hook-widget line-init _golden_zle_log
  add-zle-hook-widget line-pre-redraw _golden_zle_log
fi
"#;

/// zsh-autosuggestions' display mechanism without the plugin: a `line-pre-redraw` hook puts the
/// rest of a matching history entry in `POSTDISPLAY` and paints it with
/// `ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE` through `region_highlight`, which is exactly how the plugin
/// renders its ghost text. The integration reports the style in `ZshAutosuggestionColor`, and the
/// pty binary is supposed to leave cells in that color out of the buffer.
const AUTOSUGGEST: &str = r#"ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE='fg=8'
_golden_history=('echo hello world' 'git status --short')
_golden_suggest() {
  POSTDISPLAY=
  region_highlight=()
  if (( CURSOR == ${#BUFFER} && ${#BUFFER} )); then
    local entry
    for entry in $_golden_history; do
      if [[ $entry == "$BUFFER"?* ]]; then
        POSTDISPLAY=${entry#"$BUFFER"}
        region_highlight=("${#BUFFER} $(( ${#BUFFER} + ${#POSTDISPLAY} )) $ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE")
        break
      fi
    done
  fi
}
autoload -Uz add-zle-hook-widget
add-zle-hook-widget line-pre-redraw _golden_suggest
"#;

const CTRL_A: &str = "\x01";
const CTRL_B: &str = "\x02";
const CTRL_E: &str = "\x05";
const CTRL_F: &str = "\x06";
const CTRL_U: &str = "\x15";
const BACKSPACE: &str = "\x7f";

pub const CASES: &[Case] = &[
    Case {
        name: "fig_plain",
        columns: 80,
        screen_lines: 24,
        zshrc: "",
        steps: &[
            Step::Type("echo hello"),
            Step::Type(CTRL_B),
            Step::Type(CTRL_B),
            Step::Type(CTRL_B),
            Step::Type("X"),
            Step::Type(CTRL_A),
            Step::Type(CTRL_E),
            Step::Type(BACKSPACE),
            Step::Write("\r"),
            Step::Type("ls -la"),
        ],
    },
    Case {
        name: "fig_multiline_prompt",
        columns: 80,
        screen_lines: 24,
        zshrc: "PS1=$'multi line prompt\\n%# '\n",
        steps: &[
            Step::Type("echo hello"),
            Step::Type(CTRL_B),
            Step::Type(CTRL_B),
            Step::Write("\r"),
            Step::Type("ls"),
        ],
    },
    Case {
        name: "fig_rprompt",
        columns: 80,
        screen_lines: 24,
        zshrc: "RPS1='[right]'\n",
        steps: &[
            Step::Type("echo hello"),
            Step::Type(CTRL_A),
            Step::Type(CTRL_E),
            // Runs into the right prompt, which zsh then drops, and wraps.
            Step::Type(" 0123456789012345678901234567890123456789012345678901234567890123456789"),
        ],
    },
    Case {
        name: "fig_emoji",
        columns: 80,
        screen_lines: 24,
        zshrc: "PS1=$'\\U1F680 %# '\n",
        steps: &[
            Step::Type("echo \u{1F600} ok"),
            Step::Type(CTRL_B),
            Step::Type(CTRL_B),
            Step::Type(CTRL_B),
            Step::Type(CTRL_B),
            Step::Type("\u{00E9}"),
            Step::Type(CTRL_E),
        ],
    },
    Case {
        name: "fig_wrap",
        columns: 40,
        screen_lines: 12,
        zshrc: "",
        steps: &[
            Step::Type("echo 0123456789 abcdefghij klmnopqrst uvwxyz ABCDEFGHIJ"),
            Step::Type(CTRL_A),
            Step::Type(CTRL_F),
            Step::Type(CTRL_F),
            Step::Type(CTRL_E),
            Step::Type(BACKSPACE),
            Step::Type(BACKSPACE),
        ],
    },
    Case {
        name: "fig_autosuggestion",
        columns: 80,
        screen_lines: 24,
        zshrc: AUTOSUGGEST,
        steps: &[
            Step::Type("echo h"),
            Step::Type(CTRL_B),
            Step::Type(CTRL_E),
            Step::Type("i"),
            Step::Type(CTRL_U),
            Step::Type("git"),
        ],
    },
    Case {
        name: "fig_scroll_off",
        columns: 40,
        screen_lines: 8,
        zshrc: "",
        steps: &[
            // Puts the next prompt on the bottom row.
            Step::Write("print -l {1..10}\r"),
            // Three rows' worth: the screen scrolls twice while the line is being typed.
            Step::Type("echo 0123456789 abcdefghij 0123456789 abcdefghij 0123456789 abcdefghij 0123456789 abcd"),
            Step::Type(CTRL_A),
            Step::Type(CTRL_E),
        ],
    },
    // Not one of the ten in docs/15-architecture-study.md §4: a line taller than the screen. zsh
    // then draws only a window of the buffer, marked `>....` at the top and `<....` at the
    // bottom, and the pty binary (one line of scrollback, anchored on the prompt) has only that
    // window to read.
    Case {
        name: "fig_line_taller_than_screen",
        columns: 20,
        screen_lines: 6,
        zshrc: "",
        steps: &[
            Step::Write("print -l {1..8}\r"),
            Step::Type("echo 0123456789 abcdefghij"),
            // Typeahead: one write, one redraw.
            Step::Write("0123456789 abcdefghij 0123456789 abcdefghij 0123456789 abcdefghij 0123456789 abcdefghij"),
            Step::Type(" 0123456789 abcdefghij"),
            Step::Type(CTRL_A),
            Step::Type(CTRL_E),
        ],
    },
    Case {
        name: "fig_resize_reflow",
        columns: 80,
        screen_lines: 24,
        zshrc: "",
        steps: &[
            Step::Type("echo the quick brown fox jumps over the lazy dog"),
            Step::Resize {
                columns: 30,
                screen_lines: 24,
            },
            Step::Type(" again"),
            Step::Resize {
                columns: 100,
                screen_lines: 24,
            },
            Step::Type("!"),
        ],
    },
    Case {
        name: "fig_ps2",
        columns: 80,
        screen_lines: 24,
        zshrc: "",
        steps: &[
            Step::Type("echo \"abc"),
            Step::Write("\r"),
            Step::Type("def\""),
            Step::Write("\r"),
            Step::Type("ok"),
        ],
    },
    Case {
        name: "fig_paste",
        columns: 80,
        screen_lines: 24,
        zshrc: "",
        steps: &[
            Step::Paste("echo one\necho two"),
            Step::Type(" three"),
            Step::Type(CTRL_A),
        ],
    },
];

pub fn case(name: &str) -> Option<&'static Case> {
    CASES.iter().find(|case| case.name == name)
}

/// `crates/figterm/tests/ref`.
pub fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/figterm/tests/ref")
}

/// One write to the pty master, as it is split into steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Bytes(String),
    Resize { columns: u16, screen_lines: u16 },
}

impl Input {
    fn to_json(&self) -> Value {
        match self {
            Input::Bytes(bytes) => json!({ "input": bytes }),
            Input::Resize { columns, screen_lines } => {
                json!({ "resize": { "columns": columns, "screen_lines": screen_lines } })
            },
        }
    }
}

impl Case {
    /// Every write this case makes, in order.
    pub fn inputs(&self) -> Vec<Input> {
        self.steps
            .iter()
            .flat_map(|step| match *step {
                Step::Type(text) => text.chars().map(|c| Input::Bytes(c.to_string())).collect(),
                Step::Write(text) => vec![Input::Bytes(text.into())],
                Step::Paste(text) => vec![Input::Bytes(format!("\x1b[200~{text}\x1b[201~"))],
                Step::Resize { columns, screen_lines } => vec![Input::Resize { columns, screen_lines }],
            })
            .collect()
    }

    /// Writes `.zshenv` and `.zshrc` into `zdotdir`. `pre_integration` also loads the
    /// integration's `pre` half, which is what `exec`s the pty binary; the recorder leaves it out
    /// because it records the shell the pty binary would be reading from.
    pub fn write_zdotdir(&self, zdotdir: &Path, cli: &Path, pre_integration: bool, zle_log: bool) -> Result<()> {
        std::fs::create_dir_all(zdotdir)?;
        // `/etc/zprofile` and `/etc/zshrc` differ between machines and set their own prompt.
        std::fs::write(zdotdir.join(".zshenv"), "setopt no_global_rcs\n")?;
        let init = |when: &str| format!("eval \"$('{}' init zsh {when} --rcfile zshrc)\"\n", cli.display());
        let mut rc = String::new();
        if pre_integration {
            rc.push_str(&init("pre"));
        }
        rc.push_str(PRELUDE);
        if zle_log {
            rc.push_str(ZLE_LOG);
        }
        rc.push_str(self.zshrc);
        rc.push_str(&init("post"));
        std::fs::write(zdotdir.join(".zshrc"), rc)?;
        Ok(())
    }
}

/// The environment every golden shell starts from; nothing is inherited from the test runner.
pub fn apply_env(cmd: &mut CommandBuilder, home: &Path, zdotdir: &Path, tmpdir: &Path) {
    cmd.env_clear();
    cmd.env("HOME", home);
    cmd.env("ZDOTDIR", zdotdir);
    cmd.env("TMPDIR", tmpdir);
    cmd.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    cmd.env("TERM", TERM);
    cmd.env("LANG", "en_US.UTF-8");
    cmd.env("LC_ALL", "en_US.UTF-8");
    cmd.env("USER", "golden");
    cmd.env("LOGNAME", "golden");
    cmd.env("SHELL", ZSH);
}

/// A recording read back from `crates/figterm/tests/ref/<case>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBuffer {
    pub text: String,
    pub cursor: i64,
}

/// The expected edit buffer after the last step of `case`, from `edit_buffer.json`.
pub fn final_edit_buffer(case: &Case) -> Result<Option<EditBuffer>> {
    let path = ref_dir().join(case.name).join("edit_buffer.json");
    let steps: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let last = steps.last().context("edit_buffer.json is empty")?;
    Ok(match &last["edit_buffer"] {
        Value::Null => None,
        buffer => Some(EditBuffer {
            text: buffer["text"].as_str().context("text")?.into(),
            cursor: buffer["cursor"].as_i64().context("cursor")?,
        }),
    })
}

enum Recorded {
    Output(Vec<u8>),
    Input(Input),
}

/// zsh's `$PREBUFFER$BUFFER` and `$CURSOR`, as an edit-buffer hook would carry them.
fn parse_zle_log(log: &[u8]) -> Result<Vec<EditBuffer>> {
    log.split(|&b| b == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let record = std::str::from_utf8(record).context("zle log is not UTF-8")?;
            let mut fields = record.splitn(3, '\t');
            let (Some(prebuffer), Some(cursor), Some(text)) = (fields.next(), fields.next(), fields.next()) else {
                bail!("malformed zle log record {record:?}");
            };
            let chars = prebuffer.parse::<usize>()? + cursor.parse::<usize>()?;
            let cursor = text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i);
            Ok(EditBuffer {
                text: text.into(),
                cursor: cursor as i64,
            })
        })
        .collect()
}

/// Records `case` into `out` (normally `ref_dir().join(case.name)`), replacing what is there.
pub fn record(case: &Case, cli: &Path, out: &Path) -> Result<()> {
    let tempdir = tempfile::TempDir::new()?;
    let home = tempdir.path().join("home");
    let zdotdir = tempdir.path().join("zdotdir");
    let zle_log = tempdir.path().join("zle.log");
    std::fs::create_dir_all(&home)?;
    case.write_zdotdir(&zdotdir, cli, false, true)?;

    let pair = native_pty_system().openpty(PtySize {
        rows: case.screen_lines,
        cols: case.columns,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = CommandBuilder::new(ZSH);
    cmd.arg("-i");
    apply_env(&mut cmd, &home, &zdotdir, tempdir.path());
    cmd.env("FIGTERM_SESSION_ID", SESSION_ID);
    cmd.env("GOLDEN_ZLE_LOG", &zle_log);
    // `Term::dir` `chdir`s the replaying process into whatever the prompt reports; `/` exists
    // everywhere and does not vary between recordings.
    cmd.cwd("/");
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut writer = pair.master.take_writer()?;
    let mut reader = pair.master.try_clone_reader()?;
    let (tx, rx) = mpsc::channel::<(Instant, Vec<u8>)>();
    std::thread::spawn(move || {
        let mut buf = vec![0; READ_SIZE];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                break;
            }
        }
    });

    let start = Instant::now();
    let mut timeline: Vec<(Instant, Recorded)> = Vec::new();
    let mut zle_seen = 0;
    // One entry per step, the first being "before any input".
    let mut expected: Vec<Option<EditBuffer>> = Vec::new();
    let mut current: Option<EditBuffer> = None;

    // Collects output until zsh has been quiet for `QUIET`, then takes zsh's latest edit buffer.
    let mut settle = |timeline: &mut Vec<(Instant, Recorded)>, need_zle: bool| -> Result<Option<EditBuffer>> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            match rx.recv_timeout(QUIET) {
                Ok((at, bytes)) => timeline.push((at, Recorded::Output(bytes))),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let records = parse_zle_log(&std::fs::read(&zle_log).unwrap_or_default())?;
                    if !need_zle || !records.is_empty() {
                        if let Some(last) = records.get(zle_seen..).and_then(|new| new.last()) {
                            current = Some(last.clone());
                        }
                        zle_seen = records.len();
                        return Ok(current.clone());
                    }
                },
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("{}: zsh exited", case.name),
            }
            if Instant::now() > deadline {
                bail!("{}: output did not settle within {STEP_TIMEOUT:?}", case.name);
            }
        }
    };

    expected.push(settle(&mut timeline, true)?);
    let inputs = case.inputs();
    for input in &inputs {
        let at = Instant::now();
        match input {
            Input::Bytes(bytes) => {
                writer.write_all(bytes.as_bytes())?;
                writer.flush()?;
            },
            Input::Resize { columns, screen_lines } => {
                pair.master.resize(PtySize {
                    rows: *screen_lines,
                    cols: *columns,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
            },
        }
        timeline.push((at, Recorded::Input(input.clone())));
        expected.push(settle(&mut timeline, false)?);
    }

    child.kill().ok();
    child.wait().ok();

    // The reader thread stamps a chunk after `read` returns and the writer before `write`, so
    // sorting by time puts every input ahead of the output it caused.
    timeline.sort_by_key(|(at, _)| *at);

    std::fs::create_dir_all(out)?;
    let mut recording = Vec::new();
    let mut events = Vec::new();
    for (at, event) in &timeline {
        let t_us = at.saturating_duration_since(start).as_micros() as u64;
        events.push(match event {
            Recorded::Output(bytes) => {
                recording.extend_from_slice(bytes);
                json!({ "t_us": t_us, "output": bytes.len() })
            },
            Recorded::Input(input) => {
                let mut value = input.to_json();
                value["t_us"] = json!(t_us);
                value
            },
        });
    }
    std::fs::write(out.join("alacritty.recording"), &recording)?;
    std::fs::write(
        out.join("size.json"),
        format!(
            "{}\n",
            json!({ "columns": case.columns, "screen_lines": case.screen_lines })
        ),
    )?;
    std::fs::write(
        out.join("config.json"),
        format!(
            "{}\n",
            json!({ "history_size": HISTORY_SIZE, "session_id": SESSION_ID })
        ),
    )?;
    std::fs::write(out.join("events.json"), json_lines(&events))?;

    let steps: Vec<Value> = std::iter::once(Value::Null)
        .chain(inputs.iter().map(Input::to_json))
        .zip(&expected)
        .map(|(after, buffer)| {
            json!({
                "after": after,
                "edit_buffer": buffer.as_ref().map(|b| json!({ "text": b.text, "cursor": b.cursor })),
            })
        })
        .collect();
    std::fs::write(out.join("edit_buffer.json"), json_lines(&steps))?;
    Ok(())
}

/// A JSON array with one element per line, so a re-recording diffs line by line. DEL is escaped
/// like the other control characters; serde_json leaves it raw, which is valid but invisible.
fn json_lines(values: &[Value]) -> String {
    let mut out = String::from("[\n");
    for (i, value) in values.iter().enumerate() {
        out.push_str("  ");
        out.push_str(&value.to_string().replace('\x7f', "\\u007f"));
        out.push_str(if i + 1 == values.len() { "\n" } else { ",\n" });
    }
    out.push_str("]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zle_log_cursor_is_a_byte_offset_into_prebuffer_and_buffer() {
        let log = "0\t2\tab\0\
                   5\t1\techo\ndef\0\
                   0\t6\techo \u{1F600} ok\0";
        assert_eq!(parse_zle_log(log.as_bytes()).unwrap(), vec![
            EditBuffer {
                text: "ab".into(),
                cursor: 2
            },
            EditBuffer {
                text: "echo\ndef".into(),
                cursor: 6
            },
            EditBuffer {
                text: "echo \u{1F600} ok".into(),
                cursor: 9
            },
        ]);
    }

    #[test]
    fn case_names_are_unique_and_prefixed() {
        let mut names: Vec<_> = CASES.iter().map(|case| case.name).collect();
        assert!(names.iter().all(|name| name.starts_with("fig_")));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CASES.len());
    }
}
