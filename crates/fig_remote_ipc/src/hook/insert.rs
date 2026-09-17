//! `InsertText` for a shell that edits its own buffer.
//!
//! The pty binary carries an insertion out by typing it: it turns the request into the bytes a
//! user would press (`InsertTextRequest::to_term_string` — arrows for `offset`, backspaces for
//! `deletion`, the text, a carriage return for `immediate`), and the autocomplete app itself puts
//! backspaces and `ESC [ D` into the text. Before that it compares the buffer the app based the
//! insertion on with the one on screen and types the difference. A hook session gets the same
//! keystrokes computed the same way, then replayed here as [`Insert`] edits of `LBUFFER`, which
//! is what `fig.zsh` can apply atomically.

use fig_proto::figterm::InsertTextRequest;
use tracing::debug;

use super::wire::Insert;

enum Op {
    Backspace,
    Left,
    Right,
    Accept,
    Char(char),
}

/// The keystrokes the pty binary would have typed for `request` while the shell's buffer was
/// `buffer`.
fn keystrokes(request: &InsertTextRequest, buffer: &str) -> String {
    let mut typed = String::new();
    if let Some(expected) = request.insertion_buffer.as_deref()
        && buffer != expected
    {
        // Counted in characters: the pty binary counts bytes, which over-deletes after an emoji.
        if let Some(extra) = buffer.strip_prefix(expected) {
            typed.extend(std::iter::repeat_n('\x08', extra.chars().count()));
        } else if let Some(missing) = expected.strip_prefix(buffer) {
            typed.push_str(missing);
        }
    }
    typed.push_str(&request.to_term_string());
    typed
}

fn ops(typed: &str) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut chars = typed.chars().peekable();
    while let Some(c) = chars.next() {
        ops.push(match c {
            '\x08' | '\x7f' => Op::Backspace,
            '\r' | '\n' => Op::Accept,
            '\x1b' => {
                // A CSI sequence: only the arrows mean anything here, the rest is dropped whole.
                if chars.next_if_eq(&'[').is_none() {
                    continue;
                }
                let last = chars.by_ref().find(|c| ('\x40'..='\x7e').contains(c));
                match last {
                    Some('D') => Op::Left,
                    Some('C') => Op::Right,
                    _ => continue,
                }
            },
            c => Op::Char(c),
        });
    }
    ops
}

/// The edits that leave zsh's buffer as the pty binary's keystrokes would have — except that the
/// line is not run: an Enter ends the edits, and what follows it would in a pty have gone to the
/// command it started (see [`super::wire`] for why a hook cannot run it).
pub fn edits(request: &InsertTextRequest, buffer: &str) -> Vec<Insert> {
    let mut edits = Vec::new();
    let mut current = Insert::default();
    for op in ops(&keystrokes(request, buffer)) {
        match op {
            Op::Backspace if current.cursor_left == 0 => {
                if current.text.pop().is_none() {
                    current.delete_before += 1;
                }
            },
            Op::Char(c) if current.cursor_left == 0 => current.text.push(c),
            Op::Backspace => {
                edits.push(std::mem::take(&mut current));
                current.delete_before = 1;
            },
            Op::Char(c) => {
                edits.push(std::mem::take(&mut current));
                current.text.push(c);
            },
            Op::Left => current.cursor_left += 1,
            Op::Right => current.cursor_left -= 1,
            Op::Accept => {
                debug!("an insertion asked to run the line; the hook leaves that to the user");
                break;
            },
        }
    }
    if !current.is_noop() {
        edits.push(current);
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(delete_before: usize, text: &str, cursor_left: isize) -> Insert {
        Insert {
            delete_before,
            text: text.into(),
            cursor_left,
        }
    }

    fn request(insertion: &str, insertion_buffer: Option<&str>) -> InsertTextRequest {
        InsertTextRequest {
            insertion: Some(insertion.into()),
            insertion_buffer: insertion_buffer.map(Into::into),
            ..Default::default()
        }
    }

    /// What `insertion.ts` sends: backspaces over the typed prefix, the suggestion, and arrows back
    /// to `{cursor}`.
    #[test]
    fn the_autocomplete_app_shapes() {
        assert_eq!(edits(&request("\x08\x08checkout ", Some("git ch")), "git ch"), vec![
            insert(2, "checkout ", 0)
        ]);
        assert_eq!(
            edits(&request("--message=\"\"\x1b[D", Some("git commit ")), "git commit "),
            vec![insert(0, "--message=\"\"", 1)]
        );
        // Run it: typed, not run.
        assert_eq!(edits(&request("status\n", Some("git ")), "git "), vec![insert(
            0, "status", 0
        )]);
        // Execute from the keybinding: a bare newline, so nothing to do.
        assert!(edits(&request("\n", None), "ls").is_empty());
    }

    /// The buffer moved on after the app computed the insertion.
    #[test]
    fn the_buffer_is_reconciled_first() {
        // The user typed two more characters: erase them.
        assert_eq!(edits(&request("\x08\x08checkout ", Some("git ch")), "git chXY"), vec![
            insert(4, "checkout ", 0)
        ]);
        // An emoji is one character to erase.
        assert_eq!(edits(&request("ok", Some("a")), "a\u{1F600}"), vec![insert(1, "ok", 0)]);
        // The shell is behind: type what it is missing.
        assert_eq!(edits(&request("\x08x", Some("git c")), "git "), vec![insert(0, "x", 0)]);
        // Unrelated buffers are left alone, as the pty binary does.
        assert_eq!(edits(&request("x", Some("abc")), "zzz"), vec![insert(0, "x", 0)]);
    }

    #[test]
    fn the_request_fields() {
        let request = InsertTextRequest {
            insertion: Some("hello".into()),
            deletion: Some(3),
            offset: Some(2),
            immediate: Some(true),
            ..Default::default()
        };
        // Right twice, then three backspaces there, then the text; the Enter is not run.
        assert_eq!(edits(&request, "abcdef"), vec![
            insert(0, "", -2),
            insert(3, "hello", 0),
        ]);
        let left = InsertTextRequest {
            insertion: Some("x".into()),
            offset: Some(-1),
            ..Default::default()
        };
        assert_eq!(edits(&left, "ab"), vec![insert(0, "", 1), insert(0, "x", 0)]);
    }

    #[test]
    fn multi_line_and_emoji_text_and_what_follows_an_accept() {
        assert_eq!(edits(&request("echo \u{1F600}\ntail", None), ""), vec![insert(
            0,
            "echo \u{1F600}",
            0
        )]);
        assert_eq!(edits(&request("ab\x08c", None), ""), vec![insert(0, "ac", 0)]);
        assert!(
            edits(&request("\x1b[A", None), "").is_empty(),
            "unknown sequences are dropped"
        );
        assert!(edits(&request("", None), "").is_empty());
    }
}
