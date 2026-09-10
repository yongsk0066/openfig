use std::io::{
    self,
    Read,
    Write,
};
use std::os::fd::{
    BorrowedFd,
    IntoRawFd,
};
use std::os::unix::io::{
    AsRawFd,
    FromRawFd,
    RawFd,
};
use std::os::unix::process::CommandExt;
use std::path::Path;

use anyhow::{
    Context,
    Result,
};
use async_trait::async_trait;
use filedescriptor::FileDescriptor;
use nix::fcntl::{
    FcntlArg,
    FdFlag,
    OFlag,
    fcntl,
    open,
};
use nix::libc;
use nix::pty::{
    PtyMaster,
    Winsize,
    grantpt,
    posix_openpt,
    ptsname,
    unlockpt,
};
use nix::sys::signal::{
    SigHandler,
    Signal,
    signal,
};
use nix::sys::stat::{
    Mode,
    umask,
};
use portable_pty::{
    Child,
    PtySize,
};
use tokio::io::unix::AsyncFd;

use crate::pty::{
    AsyncMasterPty,
    CommandBuilder,
    MasterPty,
    PtyPair,
    SlavePty,
};

nix::ioctl_write_ptr_bad!(ioctl_tiocswinsz, libc::TIOCSWINSZ, Winsize);

struct UnixSlavePty {
    name: String,
    fd: FileDescriptor,
}

struct UnixMasterPty {
    fd: PtyMaster,
}

struct UnixAsyncMasterPty {
    fd: AsyncFd<PtyMaster>,
}

/// Helper function to set the close-on-exec flag for a raw descriptor
///
/// # Safety-usable invariant
///
/// This is a *safe* function that takes a `RawFd`, so it silently requires its callers to pass
/// a descriptor they own and keep open for the duration of the call. Both call sites in
/// `open_pty` pass `master.fd`/`slave.fd`, whose owners (`PtyMaster` and `FileDescriptor`) are
/// live locals at that point, so the requirement is met by inspection. Taking a `BorrowedFd<'_>`
/// would move the obligation into the type system; see docs/13-unsafe-review.md #20.
fn cloexec(fd: RawFd) -> Result<()> {
    // SAFETY:
    // Operation: `BorrowedFd::borrow_raw(fd)`.
    // Contract (std): "the resource pointed to by `fd` must remain open for the duration of the
    // returned `BorrowedFd`, and it must not have the value `-1`".
    // Evidence: the caller obligation recorded above. Both call sites derive `fd` from an
    // `AsRawFd` impl on a live owner, so the descriptor is open and non-negative (a closed or
    // invalid owner cannot exist: `PtyMaster` and `FileDescriptor` are RAII wrappers).
    // Temporal scope: the returned `BorrowedFd` is confined to this function body, which
    // performs only two `fcntl` calls and cannot close the descriptor.
    // (nix 0.30+ takes an `AsFd` here where it used to take a `RawFd`, which is why the borrow
    // is materialised at all.)
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let flags = fcntl(fd, FcntlArg::F_GETFD)?;
    fcntl(
        fd,
        FcntlArg::F_SETFD(FdFlag::from_bits_truncate(flags) | FdFlag::FD_CLOEXEC),
    )?;
    Ok(())
}

/// Open a pseudoterminal
pub fn open_pty(pty_size: &PtySize) -> Result<PtyPair> {
    // Open a new pseudoterminal master
    // The pseudoterminal must be initialized with O_NONBLOCK since on macOS, the
    // it can not be safely set with fcntl() later on.
    // https://github.com/pkgw/stund/blob/master/tokio-pty-process/src/lib.rs#L127-L133
    let oflag = OFlag::O_RDWR | OFlag::O_NONBLOCK;
    let master_pty = posix_openpt(oflag).context("Failed to openpt")?;

    // Allow pseudoterminal pair to be generated
    grantpt(&master_pty).context("Failed to grantpt")?;
    unlockpt(&master_pty).context("Failed to unlockpt")?;

    // Get the name of the pseudoterminal
    // SAFETY:
    // Operation: `nix::pty::ptsname`, an unsafe fn.
    // Contract (nix 0.31): "`ptsname()` mutates global variables and is not thread-safe.
    // Mutating global variables is always considered `unsafe` by Rust and this usage may be
    // UB." Concretely: POSIX `ptsname(3)` returns a pointer into a single static buffer, so a
    // concurrent `ptsname` on another thread can overwrite the buffer between the call and
    // nix's copy into a `String`. macOS has no `ptsname_r`, so nix exposes no safe alternative.
    // Evidence:
    // - `open_pty` has exactly one non-test caller, `figterm::figterm_main`, which calls it at a point
    //   where the process is still single-threaded: `Cli::parse`, `fig_settings::settings::init_global`
    //   (no thread spawns — it only reads a JSON file), `SystemTerminal::new_from_stdio` and
    //   `terminal.get_screen_size` all precede it, and the first `std::thread::spawn` and the tokio
    //   `Builder::new_multi_thread` come after.
    // - No other code in this process calls `ptsname` (checked across the workspace), so even later
    //   there is no second writer of the static buffer.
    // Missing premise (UNPROVEN for the test configuration): `open_pty` is `pub` and
    // `mod tests::test_openpty` calls it. Under `cargo test`'s default threaded harness several
    // tests share a process, so the single-thread premise does not hold there; under
    // `cargo nextest` (this project's runner) each test gets its own process and it does. The
    // premise is therefore configuration-dependent. See docs/13-unsafe-review.md #21.
    let pty_name = unsafe { ptsname(&master_pty) }?;
    let slave_pty = open(Path::new(&pty_name), OFlag::O_RDWR, Mode::empty())?;

    // let termios = tcgetattr(STDIN_FILENO)
    //    .context("Failed to get terminal attributes")?;
    // tcsetattr(slave_pty, SetArg::TCSANOW, termios)?;
    let winsize = Winsize {
        ws_row: pty_size.rows,
        ws_col: pty_size.cols,
        ws_xpixel: pty_size.pixel_width,
        ws_ypixel: pty_size.pixel_height,
    };
    // SAFETY:
    // Operation: `ioctl_tiocswinsz`, generated by `nix::ioctl_write_ptr_bad!` and therefore an
    // unsafe fn wrapping `libc::ioctl(fd, TIOCSWINSZ, data)`.
    // Contract (nix `ioctl_write_ptr_bad!` + POSIX): `fd` must be an open descriptor, and
    // `data` must be a readable, correctly aligned pointer to the exact struct the request
    // expects — here `struct winsize` — for the duration of the call. The `_bad` variant means
    // the request code is used verbatim rather than computed from the type, so the
    // request/type pairing is asserted by us, not derived.
    // Evidence:
    // - `slave_pty` is the `OwnedFd` returned by `open` a few lines above and is still live, so
    //   `as_raw_fd()` is an open descriptor.
    // - `&winsize` is a reference to a live, fully initialized `nix::pty::Winsize` local, hence
    //   non-null, aligned and readable for `size_of::<Winsize>()` bytes.
    // - `nix::pty::Winsize` is nix's re-export of `libc::winsize`, which is the struct `TIOCSWINSZ` is
    //   defined over in `<sys/ttycom.h>`, so the request/type pairing holds.
    // Postcondition: the slave's window size is set; the kernel copies the struct, so `winsize`
    // may be dropped afterwards.
    unsafe { ioctl_tiocswinsz(slave_pty.as_raw_fd(), &winsize) }?;

    let master = UnixMasterPty { fd: master_pty };
    let slave = UnixSlavePty {
        name: pty_name,
        // SAFETY:
        // Operation: `FileDescriptor::from_raw_fd`, an unsafe fn (`std::os::fd::FromRawFd`
        // semantics as re-implemented by the `filedescriptor` crate).
        // Contract: the descriptor must be open and valid, and the caller must transfer sole
        // ownership — the new owner will close it exactly once on drop, so no other owner may
        // close it.
        // Evidence:
        // - `slave_pty.into_raw_fd()` *consumes* the `OwnedFd` returned by `nix::fcntl::open` and suppresses its `Drop`
        //   (std `IntoRawFd`: "consumes this object, returning the raw underlying file descriptor... the caller is
        //   responsible for closing it"). That is the ownership transfer, performed by the type system rather than
        //   asserted.
        // - The descriptor is open: `open` returned `Ok` and nothing has closed it since; the `ioctl` above does not
        //   close on failure (and on failure `?` returns before here).
        // Postcondition: the `UnixSlavePty`'s `FileDescriptor` is the sole owner. The
        // `cloexec(slave.fd.as_raw_fd())` call below is placed *after* this construction
        // precisely so that an error there still drops the pty and closes the descriptor once.
        fd: unsafe { FileDescriptor::from_raw_fd(slave_pty.into_raw_fd()) },
    };

    // Ensure that these descriptors will get closed when we execute
    // the child process. This is done after constructing the Pty
    // instances so that we ensure that the Ptys get drop()'d if
    // the cloexec() functions fail (unlikely!).
    cloexec(master.fd.as_raw_fd())?;
    cloexec(slave.fd.as_raw_fd())?;

    Ok(PtyPair {
        master: Box::new(master),
        slave: Box::new(slave),
    })
}

impl SlavePty for UnixSlavePty {
    fn spawn_command(&self, builder: CommandBuilder) -> anyhow::Result<Box<dyn Child + Send + Sync>> {
        let configured_mask = builder.umask;
        let mut cmd = builder.as_command()?;

        cmd.stdin(self.fd.as_stdio()?)
            .stdout(self.fd.as_stdio()?)
            .stderr(self.fd.as_stdio()?);

        // Snapshot the descriptors we may have leaked into the child *here*, in the parent.
        // Enumerating them after `fork` is what `portable_pty::unix::close_random_fds` does, and
        // that is the one async-signal-safety violation in this `pre_exec` closure: it calls
        // `read_dir("/dev/fd")` and builds `String`s and a `Vec`, all of which allocate. This
        // process is multi-threaded at that point (tracing-appender's writer is running before the
        // fork, and tokio's runtime starts right after), so a thread holding the allocator lock at
        // `fork` leaves the child with a locked allocator and nobody to unlock it — the child
        // hangs between `fork` and `exec` and the user gets a shell that never starts. See
        // docs/13-unsafe-review.md #22.
        //
        // Iterating a pre-built `Vec` in the child allocates nothing, and the closure is only ever
        // *called* in the child (never dropped: `exec` replaces the image, and the error path
        // `_exit`s), so the `Vec`'s destructor never runs there either. Walking
        // `3..sysconf(_SC_OPEN_MAX)` instead is not viable: `OPEN_MAX` is 1,048,576 on macOS, so
        // that would add a million `close` syscalls to every shell launch.
        //
        // This is also strictly more conservative than `close_random_fds`: descriptors created by
        // `Command::spawn` after this point — notably the CLOEXEC pipe std uses to report `exec`
        // failures back to the parent — are not in the snapshot and stay open.
        let inherited_fds: Vec<RawFd> = match std::fs::read_dir("/dev/fd") {
            Ok(dir) => dir
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok()?.parse::<RawFd>().ok())
                .filter(|fd| *fd > 2)
                .collect(),
            Err(err) => {
                tracing::warn!(%err, "Failed to enumerate /dev/fd; not closing inherited descriptors");
                Vec::new()
            },
        };

        let pre_exec_fn = move || {
            // Clean up a few things before we exec the program
            // Clear out any potentially problematic signal
            // dispositions that we might have inherited
            for signo in [
                Signal::SIGCHLD,
                Signal::SIGHUP,
                Signal::SIGINT,
                Signal::SIGQUIT,
                Signal::SIGTERM,
                Signal::SIGALRM,
            ] {
                // SAFETY:
                // Operation: `nix::sys::signal::signal`, an unsafe fn.
                // Contract (nix 0.31): the handler installed must be safe to run in a signal
                // context, and the call mutates process-global disposition state.
                // Evidence: the handler is `SigHandler::SigDfl`, i.e. the kernel default — no
                // Rust code runs, so there is no signal-safety obligation at all. The global
                // mutation is confined to the freshly forked child (see `pre_exec`'s proof
                // below), which has exactly one thread, so no other thread can observe a torn
                // disposition. `sigaction(2)` is itself async-signal-safe.
                unsafe { signal(signo, SigHandler::SigDfl) }?;
            }

            // Establish ourselves as a session leader.
            nix::unistd::setsid()?;

            // Clippy wants us to explicitly cast TIOCSCTTY using
            // type::from(), but the size and potentially signedness
            // are system dependent, which is why we're using `as _`.
            // Suppress this lint for this section of code.
            {
                // Set the pty as the controlling terminal.
                // Failure to do this means that delivery of
                // SIGWINCH won't happen when we resize the
                // terminal, among other undesirable effects.
                // SAFETY:
                // Operation: the `libc::ioctl` variadic FFI call.
                // Contract (POSIX/Darwin `ioctl(fd, request, ...)`): `fd` must be open, and the
                // variadic argument must match what `request` expects. `TIOCSCTTY` takes an
                // `int` argument (0 = do not steal the terminal from another session), not a
                // pointer, so there is no pointer-validity obligation.
                // Evidence:
                // - fd 0 is the pty slave: `cmd.stdin(self.fd.as_stdio()?)` above dup2s the slave onto descriptor 0
                //   before `exec`, and `std::process::Command` performs those dup2s *before* running `pre_exec`
                //   closures.
                // - `setsid()` was called on the preceding line, so this process is a session leader with no
                //   controlling terminal — the precondition `TIOCSCTTY` requires.
                // - `libc::ioctl` is async-signal-safe (it is a bare syscall).
                // Postcondition: the pty becomes this session's controlling terminal, which is
                // what makes SIGWINCH delivery work.
                if unsafe { libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 } {
                    return Err(io::Error::last_os_error());
                }
            }

            for &fd in &inherited_fds {
                // SAFETY:
                // Operation: `libc::close`.
                // Contract (POSIX `close(2)`): none beyond passing an `int`; an invalid or
                // already-closed descriptor is reported as `EBADF`, not undefined behaviour.
                // Evidence: `close(2)` is on the POSIX async-signal-safe list, and the loop reads
                // a `Vec` that was built in the parent, so nothing here allocates.
                // Postcondition: the descriptor is closed in this child only; the parent's copy is
                // unaffected.
                unsafe { libc::close(fd) };
            }

            if let Some(mode) = configured_mask {
                umask(mode);
            }

            Ok(())
        };

        // SAFETY:
        // Operation: `std::os::unix::process::CommandExt::pre_exec`, an unsafe fn.
        // Contract (std): the closure "will be run in the context of the child process after a
        // `fork`. This primarily means that any modifications made to memory on behalf of this
        // closure will **not** be visible to the parent process... it should be unlikely to
        // ever need to write this... this closure is run after `fork` but before `exec`... all
        // the caveats of `fork` apply: the child must only call async-signal-safe functions,
        // because it may be sharing an address space with a parent that holds locks taken by
        // other threads at fork time (notably the allocator's)."
        // Evidence, operation by operation inside `pre_exec_fn`:
        // - `signal(signo, SigHandler::SigDfl)`: `sigaction(2)`, async-signal-safe.
        // - `nix::unistd::setsid()`: `setsid(2)`, async-signal-safe.
        // - `libc::ioctl(0, TIOCSCTTY, 0)`: a bare syscall, async-signal-safe.
        // - `for &fd in &inherited_fds { libc::close(fd) }`: `close(2)` is async-signal-safe and the
        //   descriptor list was built in the parent (see the comment above the snapshot), so the loop
        //   allocates nothing. This replaces `portable_pty::unix::close_random_fds()`, which violated the
        //   contract: it calls `std::fs::read_dir("/dev/fd")` and builds `String`s and a `Vec` *in the
        //   child*, and `malloc` is not async-signal-safe.
        // - `umask(mode)`: `umask(2)`, async-signal-safe.
        // - Error construction on the failure paths: `io::Error::last_os_error()` and the `Errno ->
        //   io::Error` conversion both build from a raw OS error code and do not allocate. `?` on the `nix`
        //   results likewise does not allocate.
        // - The closure captures only `configured_mask: Option<Mode>`, a `Copy` value, so no destructor and
        //   no heap access is reachable through the capture.
        // - Nothing in the closure logs, panics, or takes a lock.
        // Postcondition: on `Err`, `std::process::Command` reports the error to the parent over
        // its CLOEXEC pipe and `_exit`s the child; the parent's `spawn()` returns that error.
        // - The closure also captures `inherited_fds: Vec<RawFd>`. It is only ever *called* in the child,
        //   never dropped there (`exec` replaces the image; the error path `_exit`s), so `Vec::drop` —
        //   which would call `free` — is unreachable in the child.
        // Note this is the *parent* obligation, and the parent is multi-threaded at the fork. Contrary
        // to what docs/13-unsafe-review.md #21/#22 say, tokio is *not* what makes it multi-threaded:
        // `Runtime::build()` runs after this fork. The thread that does exist is tracing-appender's
        // writer, started by `initialize_logging` before `figterm_main` spawns the shell. (r2d2's three
        // connection-pool workers used to be here too, until `fig_settings` dropped the pool.) The
        // conclusion is unchanged: this analysis cannot be skipped.
        unsafe { cmd.pre_exec(pre_exec_fn) };

        let mut child = cmd.spawn()?;

        // Ensure that we close out the slave fds that Child retains;
        // they are not what we need (we need the master side to reference
        // them) and won't work in the usual way anyway.
        // In practice these are None, but it seems best to be move them
        // out in case the behavior of Command changes in the future.
        child.stdin.take();
        child.stdout.take();
        child.stderr.take();

        Ok(Box::new(child))
    }

    fn get_name(&self) -> Option<String> {
        Some(self.name.clone())
    }
}

#[async_trait]
impl AsyncMasterPty for UnixAsyncMasterPty {
    async fn read(&mut self, buff: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable_mut().await?;

            match guard.try_io(|inner| inner.get_mut().read(buff)) {
                Ok(result) => return result,
                Err(_would_block) => (),
            }
        }
    }

    async fn write(&mut self, buff: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.writable_mut().await?;

            match guard.try_io(|inner| inner.get_mut().write(buff)) {
                Ok(result) => return result,
                Err(_would_block) => (),
            }
        }
    }

    fn resize(&self, size: PtySize) -> Result<()> {
        let ws_size = Winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };

        let fd = self.fd.as_raw_fd();
        // SAFETY:
        // Operation: the `libc::ioctl` variadic FFI call.
        // Contract (POSIX/Darwin): `fd` open; the variadic argument must be a readable,
        // correctly aligned pointer to the struct `request` expects, valid for the call.
        // `TIOCSWINSZ` expects `const struct winsize *`.
        // Evidence:
        // - `fd` comes from `self.fd.as_raw_fd()` where `self.fd: AsyncFd<PtyMaster>` owns the master
        //   descriptor and is borrowed for `&self`, so it is open for this call.
        // - `&ws_size as *const _` points at a live, fully initialized `nix::pty::Winsize` local (=
        //   `libc::winsize`), non-null and aligned, and `ws_size` outlives the call.
        // - The `as *const _` infers `*const Winsize`; the variadic call passes it as a pointer, matching
        //   the C prototype.
        // Postcondition: the kernel copies the struct and may raise SIGWINCH in the pty's
        // foreground process group; `ws_size` may be dropped afterwards.
        let res = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws_size as *const _) };

        if res != 0 {
            anyhow::bail!("failed to ioctl(TIOCSWINSZ): {:?}", io::Error::last_os_error());
        }

        Ok(())
    }
}

impl MasterPty for UnixMasterPty {
    fn get_async_master_pty(self: Box<Self>) -> Result<Box<dyn AsyncMasterPty + Send + Sync>> {
        Ok(Box::new(UnixAsyncMasterPty {
            fd: AsyncFd::new(self.fd).context("Failed to create AsyncFd")?,
        }))
    }
}

impl AsRawFd for UnixMasterPty {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openpty() {
        open_pty(&PtySize::default()).unwrap();
    }
}
