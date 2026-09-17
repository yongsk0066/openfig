//! An abstraction over a terminal device

use std::fmt::Display;

use anyhow::Result;
use bytes::Bytes;
use flume::Receiver;
use num_traits::NumCast;

use crate::input::InputEvent;

pub mod unix;

pub mod istty;

pub use self::unix::UnixTerminal;

/// Represents the size of the terminal screen.
/// The number of rows and columns of character cells are expressed.
/// Some implementations populate the size of those cells in pixels.
// On Windows, GetConsoleFontSize() can return the size of a cell in
// logical units and we can probably use this to populate xpixel, ypixel.
// GetConsoleScreenBufferInfo() can return the rows and cols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSize {
    /// The number of rows of text
    pub rows: usize,
    /// The number of columns per row
    pub cols: usize,
    /// The width of a cell in pixels.  Some implementations never
    /// set this to anything other than zero.
    pub xpixel: usize,
    /// The height of a cell in pixels.  Some implementations never
    /// set this to anything other than zero.
    pub ypixel: usize,
}

/// Rows assumed for a terminal that reports zero of them; see [`ScreenSize::or_vt100_default`].
pub const DEFAULT_ROWS: usize = 24;
/// Columns assumed for a terminal that reports zero of them; see [`ScreenSize::or_vt100_default`].
pub const DEFAULT_COLS: usize = 80;

impl ScreenSize {
    /// Replaces a zero row or column count with the VT100's 24 rows or 80 columns.
    ///
    /// A tty nobody ever sized — no `TIOCSWINSZ` from whatever opened it (`script`, `expect`, a
    /// test harness) — reports 0×0. figterm must not build its grid from that: alacritty's grid
    /// needs at least one line and one column. With zero lines a debug build fails a
    /// `debug_assert` in `grid/storage.rs` and a release build indexes out of bounds; with zero
    /// columns a release build writes through the dangling pointer of an empty `Vec` in
    /// `Row::new` and dies with SIGSEGV. Either way the user's shell goes down with figterm.
    ///
    /// 80×24 is also what line editors assume for a tty that reports no size, and the inner pty
    /// is given the same numbers, so the shell and figterm's grid agree on the width.
    pub fn or_vt100_default(self) -> Self {
        Self {
            rows: if self.rows == 0 { DEFAULT_ROWS } else { self.rows },
            cols: if self.cols == 0 { DEFAULT_COLS } else { self.cols },
            ..self
        }
    }
}

/// Coordinates of a cell on the terminal screen.
pub struct CellCoordinate {
    pub rows: usize,
    pub cols: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocking {
    DoNotWait,
    Wait,
}

pub type InputEventResult = Vec<Result<(Option<Bytes>, InputEvent)>>;

/// `Terminal` abstracts over some basic terminal capabilities.
/// If the `set_raw_mode` or `set_cooked_mode` functions are used in
/// any combination, the implementation is required to restore the
/// terminal mode that was in effect when it was created.
pub trait Terminal {
    /// Raw mode disables input line buffering, allowing data to be
    /// read as the user presses keys, disables local echo, so keys
    /// pressed by the user do not implicitly render to the terminal
    /// output, and disables canonicalization of unix newlines to CRLF.
    fn set_raw_mode(&mut self) -> Result<()>;
    fn set_cooked_mode(&mut self) -> Result<()>;

    /// Queries the current screen size, returning width, height.
    fn get_screen_size(&mut self) -> Result<ScreenSize>;

    /// Sets the current screen size
    fn set_screen_size(&mut self, size: ScreenSize) -> Result<()>;

    /// Flush any buffered output
    fn flush(&mut self) -> Result<()>;

    fn read_input(&mut self) -> Result<Receiver<InputEventResult>>;

    /// Passes through the input without any delay on input, the delay
    /// is used to batch input since some terminals will send multiple
    /// writes for a single keypress.
    fn set_immediate_mode(&mut self, immediate: bool) -> Result<()>;
}

/// `SystemTerminal` is a concrete implementation of `Terminal`.
/// Ideally you wouldn't reference `SystemTerminal` in consuming
/// code.  This type is exposed for convenience if you are doing
/// something unusual and want easier access to the constructors.
pub type SystemTerminal = UnixTerminal;

pub fn cast<T: NumCast + Display + Copy, U: NumCast>(n: T) -> Result<U> {
    num_traits::cast(n).ok_or_else(|| anyhow::anyhow!("{} is out of bounds for this system", n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_dimensions_become_the_vt100_default() {
        let size = |rows, cols| ScreenSize {
            rows,
            cols,
            xpixel: 7,
            ypixel: 14,
        };
        assert_eq!(size(0, 0).or_vt100_default(), size(24, 80));
        assert_eq!(size(0, 120).or_vt100_default(), size(24, 120));
        assert_eq!(size(40, 0).or_vt100_default(), size(40, 80));
        assert_eq!(size(40, 120).or_vt100_default(), size(40, 120));
        assert_eq!(size(1, 1).or_vt100_default(), size(1, 1));
    }

    #[test]
    fn test_cast() {
        assert_eq!(cast::<u8, u16>(0).unwrap(), 0);
        assert_eq!(cast::<u8, u16>(255).unwrap(), 255);

        assert!(cast::<u16, u8>(256).is_err());
    }

    #[tokio::test]
    #[ignore = "fails without tty"]
    async fn test_terminal() {
        let mut term = SystemTerminal::new().unwrap();
        term.set_raw_mode().unwrap();
        term.set_cooked_mode().unwrap();
        term.set_screen_size(ScreenSize {
            rows: 24,
            cols: 80,
            xpixel: 0,
            ypixel: 0,
        })
        .unwrap();
        term.flush().unwrap();
        term.read_input().unwrap();
        term.set_immediate_mode(true).unwrap();
    }
}
