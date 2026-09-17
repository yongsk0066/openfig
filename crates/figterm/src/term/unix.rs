use std::fs::OpenOptions;
use std::io::{
    Error as IoError,
    Write,
    stdin,
    stdout,
};
use std::mem;
use std::os::fd::{
    AsFd,
    BorrowedFd,
};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{
    AtomicBool,
    Ordering,
};
use std::time::Duration;

use anyhow::{
    Context,
    Result,
    bail,
};
use bytes::BytesMut;
use filedescriptor::FileDescriptor;
use flume::{
    Receiver,
    bounded,
};
use nix::libc::{
    self,
    winsize,
};
use nix::sys::termios::{
    FlushArg,
    SetArg,
    Termios,
    cfmakeraw,
    tcdrain,
    tcflush,
    tcgetattr,
    tcsetattr,
};
use tokio::io::{
    self,
    AsyncReadExt,
};
use tokio::select;
use tokio::signal::unix::SignalKind;
use tokio::time::MissedTickBehavior;
use tracing::{
    error,
    trace,
    warn,
};

use super::InputEventResult;
use crate::input::{
    InputEvent,
    InputParser,
};
use crate::term::istty::IsTty;
use crate::term::{
    ScreenSize,
    Terminal,
    cast,
};

const BUF_SIZE: usize = 4096;

pub enum Purge {
    InputQueue,
    OutputQueue,
    InputAndOutputQueue,
}

pub enum SetAttributeWhen {
    /// changes are applied immediately
    Now,
    /// Apply once the current output queue has drained
    AfterDrainOutputQueue,
    /// Wait for the current output queue to drain, then
    /// discard any unread input
    AfterDrainOutputQueuePurgeInputQueue,
}

pub trait UnixTty {
    fn get_size(&mut self) -> Result<winsize>;
    fn set_size(&mut self, size: winsize) -> Result<()>;
    fn get_termios(&mut self) -> Result<Termios>;
    fn set_termios(&mut self, termios: &Termios, when: SetAttributeWhen) -> Result<()>;
    /// Waits until all written data has been transmitted.
    fn drain(&mut self) -> Result<()>;
    fn purge(&mut self, purge: Purge) -> Result<()>;
}

pub struct TtyWriteHandle {
    fd: FileDescriptor,
    write_buffer: Vec<u8>,
}

impl TtyWriteHandle {
    fn new(fd: FileDescriptor) -> Self {
        Self {
            fd,
            write_buffer: Vec::with_capacity(BUF_SIZE),
        }
    }

    fn flush_local_buffer(&mut self) -> std::result::Result<(), IoError> {
        if !self.write_buffer.is_empty() {
            self.fd.write_all(&self.write_buffer)?;
            self.write_buffer.clear();
        }
        Ok(())
    }
}

impl Write for TtyWriteHandle {
    fn write(&mut self, buf: &[u8]) -> std::result::Result<usize, IoError> {
        if self.write_buffer.len() + buf.len() > self.write_buffer.capacity() {
            self.flush()?;
        }
        if buf.len() >= self.write_buffer.capacity() {
            self.fd.write(buf)
        } else {
            self.write_buffer.write(buf)
        }
    }

    fn flush(&mut self) -> std::result::Result<(), IoError> {
        self.flush_local_buffer()?;
        self.drain().map_err(|e| IoError::other(format!("{e}")))?;
        Ok(())
    }
}

/// Borrow a `filedescriptor::FileDescriptor` as a `std::os::fd::BorrowedFd`.
///
/// # Safety-usable invariant
///
/// The returned `BorrowedFd<'_>` borrows `fd`, so it cannot outlive the owner. This function is
/// safe and places no obligation on its callers.
///
/// (The previous `/// SAFETY:` doc comment was in the wrong place: on a safe `fn`, a `SAFETY`
/// heading reads as a caller obligation. The proof belongs inside, next to the unsafe
/// operation, and the caller-facing statement is the invariant above.)
fn borrow_fd(fd: &FileDescriptor) -> BorrowedFd<'_> {
    // SAFETY:
    // Operation: `BorrowedFd::borrow_raw(fd.as_raw_fd())`.
    // Contract (std): the resource must remain open for the lifetime of the returned
    // `BorrowedFd`, and the value must not be `-1`.
    // Evidence:
    // - `FileDescriptor` is an RAII owner (filedescriptor 0.8): it holds an open descriptor from
    //   construction until `Drop`, so `as_raw_fd()` is open and non-negative here. TYPE FACT, not an
    //   assumption about callers.
    // - Temporal scope: the elided output lifetime is tied to `&FileDescriptor`, so the borrow checker
    //   forbids the returned `BorrowedFd` from outliving the owner. This is what turns the std
    //   contract's temporal clause into a compiler-enforced fact.
    unsafe { BorrowedFd::borrow_raw(fd.as_raw_fd()) }
}

impl UnixTty for TtyWriteHandle {
    fn get_size(&mut self) -> Result<winsize> {
        // SAFETY:
        // Operation: `mem::zeroed::<winsize>()`.
        // Contract (std `mem::zeroed`): "it is undefined behavior if this is used to create an
        // invalid value" — the all-zero bit pattern must be a valid value of `T`.
        // Evidence: `libc::winsize` is `#[repr(C)]` with four `c_ushort` fields
        // (`ws_row`, `ws_col`, `ws_xpixel`, `ws_ypixel`). Every bit pattern is a valid `u16`
        // (Rust Reference, integer validity), and a `repr(C)` struct of valid fields with
        // zeroed padding is valid, so zero is a valid `winsize`.
        // Postcondition: `size` is fully initialized before the `&mut size` below, so the
        // `ioctl` never sees uninitialized storage even if it writes nothing.
        let mut size: winsize = unsafe { mem::zeroed() };
        // SAFETY:
        // Operation: the `libc::ioctl` variadic FFI call.
        // Contract (POSIX/Darwin): `fd` open; the variadic argument must match `request`.
        // `TIOCGWINSZ` expects `struct winsize *` and writes through it.
        // Evidence:
        // - `self.fd: FileDescriptor` is an RAII owner borrowed for `&mut self`, so the descriptor is open
        //   for this call.
        // - `&mut size` is a unique reference to a live, initialized `winsize` local; it is non-null,
        //   aligned and valid for writes of `size_of::<winsize>()` bytes, and no other reference to `size`
        //   is live across the call.
        // Postcondition: on a `0` return the kernel has overwritten `size`; on a non-zero
        // return `size` keeps its zeroed value, which the `bail!` discards.
        if unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCGWINSZ as _, &mut size) } != 0 {
            bail!("failed to ioctl(TIOCGWINSZ): {}", IoError::last_os_error());
        }
        Ok(size)
    }

    fn set_size(&mut self, size: winsize) -> Result<()> {
        // SAFETY:
        // Operation: the `libc::ioctl` variadic FFI call.
        // Contract (POSIX/Darwin): `fd` open; `TIOCSWINSZ` expects a readable
        // `const struct winsize *`.
        // Evidence:
        // - `self.fd` is an open RAII-owned descriptor borrowed for `&mut self`.
        // - `&size as *const _` points at the by-value `winsize` parameter, a live and fully initialized
        //   local, non-null and aligned, readable for the whole call.
        // Postcondition: the kernel copies the struct; `size` may be dropped afterwards.
        if unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ as _, &size as *const _) } != 0 {
            bail!("failed to ioctl(TIOCSWINSZ): {:?}", IoError::last_os_error());
        }

        Ok(())
    }

    fn get_termios(&mut self) -> Result<Termios> {
        tcgetattr(borrow_fd(&self.fd)).context("get_termios failed")
    }

    fn set_termios(&mut self, termios: &Termios, when: SetAttributeWhen) -> Result<()> {
        let when = match when {
            SetAttributeWhen::Now => SetArg::TCSANOW,
            SetAttributeWhen::AfterDrainOutputQueue => SetArg::TCSADRAIN,
            SetAttributeWhen::AfterDrainOutputQueuePurgeInputQueue => SetArg::TCSAFLUSH,
        };
        tcsetattr(borrow_fd(&self.fd), when, termios).context("set_termios failed")
    }

    fn drain(&mut self) -> Result<()> {
        tcdrain(borrow_fd(&self.fd)).context("tcdrain failed")
    }

    fn purge(&mut self, purge: Purge) -> Result<()> {
        let param = match purge {
            Purge::InputQueue => FlushArg::TCIFLUSH,
            Purge::OutputQueue => FlushArg::TCOFLUSH,
            Purge::InputAndOutputQueue => FlushArg::TCIOFLUSH,
        };
        tcflush(borrow_fd(&self.fd), param).context("tcflush failed")
    }
}

/// A unix style terminal
pub struct UnixTerminal {
    write: TtyWriteHandle,
    saved_termios: Termios,
}

impl UnixTerminal {
    /// Attempt to create an instance from the stdin and stdout of the
    /// process.  This will fail unless both are associated with a tty.
    /// Note that this will duplicate the underlying file descriptors
    /// and will no longer participate in the stdin/stdout locking
    /// provided by the rust standard library.
    pub fn new_from_stdio() -> Result<UnixTerminal> {
        Self::new_with(&stdin(), &stdout())
    }

    pub fn new_with<A: AsRawFd + AsFd, B: AsRawFd + AsFd>(read: &A, write: &B) -> Result<UnixTerminal> {
        if !read.is_tty() || !write.is_tty() {
            anyhow::bail!("stdin and stdout must both be tty handles");
        }

        let mut write = TtyWriteHandle::new(FileDescriptor::dup(write)?);
        let saved_termios = write.get_termios()?;

        Ok(UnixTerminal { write, saved_termios })
    }

    /// Attempt to explicitly open a handle to the terminal device
    /// (/dev/tty) and build a `UnixTerminal` from there.  This will
    /// yield a terminal even if the stdio streams have been redirected,
    /// provided that the process has an associated controlling terminal.
    pub fn new() -> Result<UnixTerminal> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        Self::new_with(&file, &file)
    }
}

static IMMEDIATE_MODE: AtomicBool = AtomicBool::new(true);

impl Terminal for UnixTerminal {
    fn set_raw_mode(&mut self) -> Result<()> {
        let mut raw = self.write.get_termios()?;
        cfmakeraw(&mut raw);
        self.write
            .set_termios(&raw, SetAttributeWhen::AfterDrainOutputQueuePurgeInputQueue)
            .context("failed to set raw mode")?;
        self.write.flush()?;

        Ok(())
    }

    fn set_cooked_mode(&mut self) -> Result<()> {
        self.write.set_termios(&self.saved_termios, SetAttributeWhen::Now)
    }

    fn get_screen_size(&mut self) -> Result<ScreenSize> {
        let size = self.write.get_size()?;
        let reported = ScreenSize {
            rows: cast(size.ws_row)?,
            cols: cast(size.ws_col)?,
            xpixel: cast(size.ws_xpixel)?,
            ypixel: cast(size.ws_ypixel)?,
        };
        // Both the initial size and every resize come through here, so this is the one place a
        // zero dimension has to be stopped before it reaches the pty and alacritty's grid.
        let size = reported.or_vt100_default();
        if size != reported {
            warn!(
                ?reported,
                ?size,
                "The terminal reports a zero dimension; using the VT100 default"
            );
        }
        Ok(size)
    }

    fn set_screen_size(&mut self, size: ScreenSize) -> Result<()> {
        let size = winsize {
            ws_row: cast(size.rows)?,
            ws_col: cast(size.cols)?,
            ws_xpixel: cast(size.xpixel)?,
            ws_ypixel: cast(size.ypixel)?,
        };

        self.write.set_size(size)
    }

    fn flush(&mut self) -> Result<()> {
        self.write.flush().context("flush failed")
    }

    fn set_immediate_mode(&mut self, immediate: bool) -> Result<()> {
        IMMEDIATE_MODE.store(immediate, Ordering::SeqCst);
        Ok(())
    }

    fn read_input(&mut self) -> Result<Receiver<InputEventResult>> {
        let mut window_change_signal = tokio::signal::unix::signal(SignalKind::window_change())?;
        let (input_tx, input_rx) = bounded::<InputEventResult>(1);

        tokio::spawn(async move {
            let mut stdin = io::stdin();
            let mut parser = InputParser::new();
            let mut buf = BytesMut::with_capacity(crate::BUFFER_SIZE);
            // Only armed while the parser holds input it cannot decide on yet; see below.
            let mut read_timeout = tokio::time::interval(Duration::from_millis(1));
            read_timeout.set_missed_tick_behavior(MissedTickBehavior::Delay);

            macro_rules! parse_buf {
                ($maybe_more:expr) => {
                    async {
                        let mut events = vec![];
                        parser.parse(&buf, |raw, evt| events.push(Ok((raw, evt))), $maybe_more);
                        buf.clear();
                        trace!(target: crate::latency::TARGET, t_us = crate::latency::now_us(), "input_parsed");
                        if !events.is_empty()
                            && let Err(err) = input_tx.send_async(events).await
                        {
                            error!(%err, "Error sending event");
                        }
                    }
                }
            }

            loop {
                select! {
                    biased;
                    res = stdin.read_buf(&mut buf) => {
                        match res {
                            Ok(n) => {
                                trace!(target: crate::latency::TARGET, t_us = crate::latency::now_us(), n, "stdin_read");
                                trace!(buf =? &buf[0..n], "Read input");
                                let full = buf.capacity() == buf.len();
                                if full {
                                    buf.reserve(buf.len());
                                }
                                // Every key that is complete goes to the main loop now. At the
                                // prompt (not immediate mode) a terminal may split one keypress
                                // over several writes, so what could still grow — a lone ESC, a
                                // partial escape sequence, half a UTF-8 character — stays in the
                                // parser (`maybe_more`) until the next read or `read_timeout`.
                                //
                                // This used to hold *every* read until `read_timeout` ticked. The
                                // interval is 1 ms, but tokio rounds a deadline up to the end of
                                // the next millisecond, so each keystroke at the prompt waited
                                // 2.3 ms (median over 216 keys) before reaching the shell: 2.3 of
                                // the 2.55 ms a key took to echo through figterm.
                                parse_buf!(!IMMEDIATE_MODE.load(Ordering::SeqCst)).await;
                                read_timeout.reset();
                            }
                            Err(err) => {
                                if let Err(err) = input_tx.send_async(vec![Err(anyhow::anyhow!(err))]).await {
                                    error!(%err, "Error sending event");
                                }
                            }
                        }
                    }
                    _ = read_timeout.tick(), if parser.needs_flush() && !IMMEDIATE_MODE.load(Ordering::SeqCst) => {
                        tracing::debug!("Flushing held input");
                        parse_buf!(false).await;
                    }
                    _ = window_change_signal.recv() => {
                        let event = InputEvent::Resized;
                        if let Err(err) = input_tx.send_async(vec![Ok((None, event))]).await {
                            warn!(%err, "Error sending event");
                        }
                    }
                }
            }
        });

        Ok(input_rx)
    }
}

impl Drop for UnixTerminal {
    fn drop(&mut self) {
        // Restore the terminal *first*. The flush can fail (EIO once the pty is gone, EPIPE, a
        // full buffer), and it used to run first and panic, leaving the user's terminal stuck in
        // raw mode with no echo — a shell that has to be recovered with a blind `stty sane`.
        // Panicking in a `Drop` is also an abort during unwinding.
        if let Err(err) = self.write.set_termios(&self.saved_termios, SetAttributeWhen::Now) {
            warn!(%err, "Failed to restore the original termios state");
        }

        if let Err(err) = self.write.flush() {
            warn!(%err, "Failed to flush the terminal on shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::Term;
    use alacritty_terminal::ansi::Processor;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::SizeInfo;

    use super::*;

    /// A pty nobody sized reports 0×0 through `TIOCGWINSZ`. Built from that, alacritty's grid
    /// fails a `debug_assert` in debug builds and SIGSEGVs in release.
    #[test]
    fn unsized_tty_gets_a_usable_screen_size() {
        let pty = nix::pty::openpty(None, None).unwrap();
        let mut terminal = UnixTerminal::new_with(&pty.slave, &pty.slave).unwrap();

        let raw = terminal.write.get_size().unwrap();
        assert_eq!((raw.ws_row, raw.ws_col), (0, 0), "precondition: a fresh pty is 0x0");

        let size = terminal.get_screen_size().unwrap();
        assert_eq!((size.rows, size.cols), (24, 80));

        let mut term = Term::new(SizeInfo::new(size.rows, size.cols), VoidListener, 1, "0x0".into());
        let mut processor = Processor::new();
        processor.advance(&mut term, b"hello\r\nworld\r\n\x1b[2J\x1b[H$ ");
        assert!(term.get_current_buffer().is_none());
    }
}
