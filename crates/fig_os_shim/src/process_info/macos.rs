use std::ffi::OsStr;
use std::mem::MaybeUninit;
use std::os::unix::prelude::OsStrExt;
use std::path::PathBuf;
use std::sync::Weak;

use super::Pid;
use super::pid::RawPid;
use crate::Context;

pub(crate) fn current(ctx: Weak<Context>) -> Pid {
    Pid(ctx.clone(), nix::unistd::getpid().into())
}

pub(crate) fn parent(ctx: Weak<Context>, pid: &RawPid) -> Option<Box<Pid>> {
    let pid = pid.0;
    let mut info = MaybeUninit::<nix::libc::proc_bsdinfo>::zeroed();
    // SAFETY:
    // Operation: the `proc_pidinfo` FFI call.
    // Contract (Darwin `libproc.h`): `buffer` must be writable for `buffersize` bytes and, for
    // `PROC_PIDTBSDINFO`, should be `sizeof(struct proc_bsdinfo)`. The call returns the number
    // of bytes actually written, or 0/-1 on error; it writes nothing on error.
    // Evidence:
    // - `info` is a live `MaybeUninit<proc_bsdinfo>` local; `as_mut_ptr().cast()` is non-null,
    //   aligned for `proc_bsdinfo` (`MaybeUninit<T>` has `T`'s layout, std) and writable for
    //   exactly `size_of::<proc_bsdinfo>()` bytes — the same value passed as `buffersize`.
    // - `pid` is a plain `i32`; an unknown pid yields an error return, not UB.
    // - The zero-fill (`MaybeUninit::zeroed`) happens before the call, so the storage is already
    //   fully initialized regardless of what the kernel writes.
    let ret = unsafe {
        nix::libc::proc_pidinfo(
            pid,
            nix::libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<nix::libc::proc_bsdinfo>() as _,
        )
    };
    if ret as usize != std::mem::size_of::<nix::libc::proc_bsdinfo>() {
        return None;
    }
    // SAFETY:
    // Operation: `MaybeUninit::<proc_bsdinfo>::assume_init`.
    // Contract (std): the memory must be initialized with a valid value of the type.
    // Evidence, two independent arguments, either of which suffices:
    // - The storage was created with `MaybeUninit::zeroed`, and `proc_bsdinfo` is a `#[repr(C)]`
    //   struct of unsigned integers and `u8` arrays. Every bit pattern is a valid value of each
    //   field (Rust Reference, integer validity), so all-zero is a valid `proc_bsdinfo` even if the
    //   kernel wrote nothing.
    // - The `ret as usize != size_of::<proc_bsdinfo>()` guard immediately above returned early
    //   unless the kernel wrote exactly the whole struct.
    // Postcondition: `info` is an owned, initialized `proc_bsdinfo`; the `MaybeUninit` is
    // consumed, so there is no double-read concern.
    let info = unsafe { info.assume_init() };
    match info.pbi_ppid {
        0 => None,
        ppid => Some(Box::new(Pid(ctx, RawPid(ppid.try_into().ok()?)))),
    }
}

pub(crate) fn exe(_: Weak<Context>, pid: &RawPid) -> Option<PathBuf> {
    let mut buffer = [0u8; 4096];
    let pid = pid.0;
    let buffer_ptr = buffer.as_mut_ptr().cast::<std::ffi::c_void>();
    let buffer_size = buffer.len() as u32;
    // SAFETY:
    // Operation: the `proc_pidpath` FFI call.
    // Contract (Darwin `libproc.h`): `buffer` must be writable for `buffersize` bytes
    // (at least `PROC_PIDPATHINFO_MAXSIZE`); returns the number of bytes written, or 0
    // on failure.
    // Evidence: `buffer` is a live `[u8; 4096]` local, so `as_mut_ptr()` is non-null,
    // aligned (alignment 1) and writable for exactly `buffer.len()` bytes, which is the
    // value passed as `buffersize`. The array is zero-initialized, so no uninitialized
    // storage is ever exposed.
    // Robustness note (not a safety defect): the `match` below treats every non-zero
    // `ret` as a length. If `proc_pidpath` ever returned a negative value, `len as usize`
    // would be huge and the slice index would *panic* — a bounds check, not UB. See
    // docs/architecture.md (native thread ownership), adjacent smells.
    let ret = unsafe { nix::libc::proc_pidpath(pid, buffer_ptr, buffer_size) };
    match ret {
        0 => None,
        len => Some(PathBuf::from(OsStr::from_bytes(&buffer[..len as usize]))),
    }
}

pub(crate) fn cmdline(_: Weak<Context>, _: &RawPid) -> Option<String> {
    None
}
