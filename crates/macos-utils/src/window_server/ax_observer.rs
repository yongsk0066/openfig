use std::boxed::Box;
use std::ffi::c_void;
use std::pin::Pin;

use accessibility::util::ax_call;
use accessibility_sys::{
    AXError,
    AXObserverAddNotification,
    AXObserverCallback,
    AXObserverCreate,
    AXObserverGetRunLoopSource,
    AXObserverRef,
    AXUIElementRef,
    kAXErrorSuccess,
    pid_t,
};
use core_foundation::base::TCFType;
use core_foundation::runloop::{
    CFRunLoopAddSource,
    CFRunLoopGetCurrent,
    CFRunLoopRemoveSource,
    kCFRunLoopDefaultMode,
};
use core_foundation::string::CFString;

/// An `AXObserverRef` plus the heap cell that backs the `refcon` handed to the
/// HIServices callback.
///
/// # Invariants
///
/// - `inner` is a live `AXObserverRef` returned by `AXObserverCreate` (Create rule, +1). Note that
///   this crate never balances that +1: see the leak note on [`AXObserver::create`].
/// - `ax_ref` is a live `AXUIElementRef` that outlives this value. This is *not* enforced by this
///   type; see the `# Safety` contract of [`AXObserver::create`].
/// - `callback_data` is pinned for the whole lifetime of this value, and its address is the
///   `refcon` that HIServices passes back to the `AXObserverCallback`. It is therefore aliased by
///   foreign code from the first successful [`AXObserver::subscribe`] until the run-loop source is
///   removed in `Drop`.
/// - Run-loop affinity: the `CFRunLoop` that `create` ran on is the only run loop the source was
///   added to, and `Drop` must run on that same run loop. Nothing in the type system enforces this;
///   see finding #2 in `docs/13-unsafe-review.md`.
pub struct AXObserver<T> {
    inner: AXObserverRef,
    ax_ref: AXUIElementRef,
    callback_data: Pin<Box<T>>,
}

// SAFETY: UNPROVEN — see docs/13-unsafe-review.md #2.
//
// Proposition asserted: `AXObserver<T>` may be moved to another thread (`Send`) and shared by
// reference across threads (`Sync`) for every `T`.
//
// What can be proved:
// - `AXObserverRef` and `AXUIElementRef` are CFTypes. `CFRetain`/`CFRelease` are documented as
//   thread-safe (Apple, "Memory Management Programming Guide for Core Foundation"), so merely
//   transferring the pointers between threads is not itself a data race.
//
// What is NOT proved and makes this impl unsound as written:
// 1. `T` is unconstrained. `unsafe impl<T> Send for AXObserver<T>` promotes an arbitrary `!Send`
//    `T` (e.g. `Rc`, a `MainThreadMarker`-bearing value) into a `Send` value. The impl should at
//    minimum be bounded `T: Send` / `T: Sync`.
// 2. `Drop` calls `CFRunLoopRemoveSource(CFRunLoopGetCurrent(), ..)`. `CFRunLoopGetCurrent` returns
//    the *calling thread's* run loop (Apple, CFRunLoop reference). If this value is dropped on a
//    thread other than the one `create` ran on, the source stays attached to the original run loop
//    while `callback_data` is deallocated, and the next AX notification dereferences a dangling
//    `refcon` in `application_ax_callback` -> use-after-free.
//
// Today no such drop is reachable: every `create`/`deregister` path runs on the tao main thread
// (`PlatformBoundEvent::Initialize` and the `NSWorkspace` notification selectors). That is a
// project invariant that nothing enforces, not a proof of this impl.
unsafe impl<T> Send for AXObserver<T> {}
// SAFETY: UNPROVEN — see the comment on the `Send` impl above and docs/13-unsafe-review.md #2.
unsafe impl<T> Sync for AXObserver<T> {}

impl<T> AXObserver<T> {
    /// Create an observer for `pid` and attach its run-loop source to the *current* run loop.
    ///
    /// # Safety
    ///
    /// The caller must ensure all of the following.
    ///
    /// 1. `ax_ref` is a non-null `AXUIElementRef` that stays alive for at least as long as the
    ///    returned `AXObserver<T>`. `subscribe` passes it to `AXObserverAddNotification` without
    ///    retaining it, so this type does not keep it alive by itself.
    /// 2. `callback` is a function whose ABI is `AXObserverCallback` and which is prepared to be
    ///    invoked with `refcon == &*callback_data` — that is, it must reinterpret `refcon` as `*mut
    ///    T` and nothing else — for as long as the returned value lives.
    /// 3. `callback` and the returned value's `Drop` both run on the thread whose run loop is
    ///    current at this call. See the `Send`/`Sync` note above.
    /// 4. The process is AX-trusted (`AXIsProcessTrusted`), otherwise `AXObserverCreate` fails and
    ///    this returns `Err` — that is an error, not UB, but callers rely on it.
    ///
    /// If those hold, the returned value owns the `+1` reference produced by `AXObserverCreate`
    /// and a pinned `T` whose address is stable for the value's lifetime.
    ///
    /// Leak note: `Drop` removes the run-loop source but never calls `CFRelease(self.inner)`, so
    /// each observer leaks one `AXObserverRef`. See docs/13-unsafe-review.md #6.
    pub unsafe fn create(
        pid: pid_t,
        ax_ref: AXUIElementRef,
        data: T,
        callback: AXObserverCallback,
    ) -> Result<Self, AXError> {
        // SAFETY:
        // Operations: (1) `ax_call::<_, AXObserverRef>`, which is an unsafe fn that calls the
        // closure with a `*mut AXObserverRef` into uninitialized storage and then
        // `MaybeUninit::assume_init`s it; (2) the `AXObserverCreate` FFI call inside it.
        // Contract of `ax_call`: the closure must initialize `*out` whenever it returns
        // `kAXErrorSuccess`, because `ax_call` only calls `assume_init` on that path
        // (`crates/macos-utils/accessibility-master/accessibility/src/util.rs`).
        // Contract of `MaybeUninit::assume_init` (std): the memory must be initialized with a
        // valid value of the type.
        // Evidence:
        // - HIServices documents `AXObserverCreate(pid, callback, outObserver)` as writing a `+1` observer
        //   through `outObserver` iff it returns `kAXErrorSuccess`; `AXObserverRef` is a raw pointer, so
        //   any value it writes is a valid `AXObserverRef`.
        // - `callback` is a `fn` pointer with the C ABI required by `AXObserverCallback` (TYPE FACT from
        //   the `accessibility_sys` declaration), so the foreign call is ABI-correct.
        // - `pid` is a plain `i32`; an unknown pid yields `kAXErrorInvalidUIElement`, not UB.
        // Postcondition: on `Ok`, `observer` is a live `AXObserverRef` that this function now
        // owns the sole `+1` on.
        let observer = ax_call(|x: *mut AXObserverRef| AXObserverCreate(pid, callback, x))?;

        // SAFETY:
        // Operations: `CFRunLoopGetCurrent`, `AXObserverGetRunLoopSource`, `CFRunLoopAddSource`,
        // and the read of the extern static `kCFRunLoopDefaultMode`.
        // Contracts (Apple CFRunLoop / HIServices reference):
        // - `CFRunLoopGetCurrent()` never returns NULL; it returns the calling thread's run loop and
        //   follows the Get rule.
        // - `AXObserverGetRunLoopSource(observer)` follows the Get rule and requires a live observer.
        //   POSTCONDITION of the call above: `observer` is live and owned here.
        // - `CFRunLoopAddSource(rl, source, mode)` requires non-null `rl`, `source` and `mode`, and retains
        //   the source itself.
        // - `kCFRunLoopDefaultMode` is an extern static initialized by CoreFoundation before any Rust code
        //   in this process runs; reading it is the unsafe operation and it is never written by this
        //   program, so there is no data race.
        // Postcondition: the observer's source is attached to *this thread's* run loop. That
        // pairing is the premise of the `Drop` impl below and of `create`'s `# Safety` clause 3.
        CFRunLoopAddSource(
            CFRunLoopGetCurrent(),
            AXObserverGetRunLoopSource(observer),
            kCFRunLoopDefaultMode,
        );

        Ok(Self {
            inner: observer,
            ax_ref,
            callback_data: Box::pin(data),
        })
    }

    /// Register `ax_event` on the observed element, passing `&mut *self.callback_data` as the
    /// `refcon`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ax_event` is one of the `kAX*Notification` names accepted by
    /// HIServices for `self.ax_ref`, and that every precondition of [`AXObserver::create`] still
    /// holds — in particular that `self.ax_ref` is still live. On success the callback passed to
    /// `create` becomes reachable from the run loop with `refcon = &raw mut *self.callback_data`,
    /// and stays reachable until `Drop` removes the run-loop source.
    pub unsafe fn subscribe(&mut self, ax_event: &str) -> Result<(), AXError> {
        // Bound to a local so the CFString outlives the call below rather than relying on
        // temporary-lifetime rules inside an argument list.
        let name = CFString::from(ax_event);

        // SAFETY:
        // Operation: `Pin::get_unchecked_mut(self.callback_data.as_mut())`.
        // Contract (std `Pin::get_unchecked_mut`): the caller must never move the pointee out of
        // the returned `&mut T` afterwards.
        // Evidence: the reference is immediately decayed to a raw pointer and only its address is
        // used; no `T` value is read, written or moved through it here, and no other method of
        // this type moves out of `callback_data` (it is only ever dropped in place).
        // Why `get_unchecked_mut` and not `&*self.callback_data as *const T`: the `refcon` is
        // later reconstituted as `&mut AccessibilityCallbackData` in `application_ax_callback`.
        // The Rust Reference forbids writing through a pointer whose provenance derives from a
        // shared reference, so the pointer must be derived from a mutable borrow.
        let callback_data: *mut T = unsafe { Pin::get_unchecked_mut(self.callback_data.as_mut()) };

        // SAFETY:
        // Operation: the `AXObserverAddNotification` FFI call.
        // Contract (HIServices `AXObserverAddNotification(observer, element, notification,
        // refcon)`): `observer` must be a live `AXObserverRef`, `element` a live
        // `AXUIElementRef`, `notification` a live `CFStringRef`, and `refcon` an arbitrary
        // pointer that is handed back verbatim to the callback. The function does not take
        // ownership of any argument; it copies/retains what it needs.
        // Evidence:
        // - `self.inner` is live: POSTCONDITION of `create`, and nothing between `create` and here releases
        //   it (`Drop` is the only other user and has not run).
        // - `self.ax_ref` is live by `create`'s `# Safety` clause 1, restated by this function's own `#
        //   Safety` section.
        // - `name` is a live `CFString` owned by this stack frame and is not dropped until after the call
        //   returns; `as_concrete_TypeRef` follows the Get rule and does not transfer ownership
        //   (core-foundation 0.10 `TCFType`).
        // - `callback_data` is a `*mut T` into the pinned box owned by `self`, whose address is stable for
        //   `self`'s lifetime (type INVARIANT above), i.e. it stays valid for exactly as long as the
        //   registration can fire.
        // Postcondition: on `kAXErrorSuccess`, foreign code holds `callback_data` and may
        // dereference it from the run loop until `Drop` removes the source.
        //
        // This deliberately does *not* use `accessibility::util::ax_call`. That helper is generic
        // over the out-parameter type and ends in `MaybeUninit::<V>::assume_init()`. The previous
        // code instantiated it at `V = c_void`, but `AXObserverAddNotification` has no
        // out-parameter and never writes through the pointer, so `assume_init` produced an
        // uninitialized `c_void` — an invalid value, which the Rust Reference lists as undefined
        // behavior. See docs/13-unsafe-review.md #1.
        let err = unsafe {
            AXObserverAddNotification(
                self.inner,
                self.ax_ref,
                name.as_concrete_TypeRef(),
                callback_data as *mut c_void,
            )
        };

        if err == kAXErrorSuccess { Ok(()) } else { Err(err) }
    }
}

impl<T> Drop for AXObserver<T> {
    fn drop(&mut self) {
        // SAFETY:
        // Operations: `CFRunLoopGetCurrent`, `AXObserverGetRunLoopSource`,
        // `CFRunLoopRemoveSource`, and the read of the extern static `kCFRunLoopDefaultMode`.
        // Contract (Apple CFRunLoop reference): `CFRunLoopRemoveSource(rl, source, mode)`
        // requires non-null arguments; removing a source that is not attached is a documented
        // no-op.
        // Evidence:
        // - `self.inner` is live until this `drop` returns (type INVARIANT), so
        //   `AXObserverGetRunLoopSource` is valid and returns the same Get-rule source that `create`
        //   attached.
        // - `CFRunLoopGetCurrent()` is non-null on every thread.
        // Purpose of this operation: it is what stops foreign code from dereferencing the
        // `refcon` after `self.callback_data` is deallocated immediately after this `drop` body.
        //
        // UNPROVEN — see docs/13-unsafe-review.md #2. The proof above only holds if this runs on
        // the same thread as `create`; `CFRunLoopGetCurrent()` is thread-local, and the
        // `unsafe impl Send` above makes an off-thread drop expressible in safe code. On an
        // off-thread drop the source survives, `callback_data` is freed, and the next
        // notification is a use-after-free.
        //
        // Leak note: `CFRelease(self.inner)` is missing here, so the `+1` from
        // `AXObserverCreate` is never balanced. See docs/13-unsafe-review.md #6.
        unsafe {
            CFRunLoopRemoveSource(
                CFRunLoopGetCurrent(),
                AXObserverGetRunLoopSource(self.inner),
                kCFRunLoopDefaultMode,
            );
        }
    }
}
