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
    AXObserverRemoveNotification,
    AXUIElementRef,
    kAXErrorSuccess,
    pid_t,
};
use core_foundation::base::{
    CFRelease,
    CFRetain,
    TCFType,
};
use core_foundation::runloop::{
    CFRunLoopAddSource,
    CFRunLoopGetCurrent,
    CFRunLoopGetMain,
    CFRunLoopRef,
    CFRunLoopRemoveSource,
    kCFRunLoopDefaultMode,
};
use core_foundation::string::CFString;

/// An `AXObserverRef` plus the heap cell that backs the `refcon` handed to the
/// HIServices callback.
///
/// # Invariants
///
/// - `inner` is a live `AXObserverRef` returned by `AXObserverCreate` (Create rule, +1), owned by
///   this value and released in `Drop`.
/// - `ax_ref` is a live `AXUIElementRef`. This value holds its own reference on it — taken with
///   `CFRetain` in [`AXObserver::create`] and released in `Drop` — so it stays live for exactly as
///   long as the observer that names it can fire.
/// - `subscriptions` lists the notification names successfully registered through
///   [`AXObserver::subscribe`], so `Drop` can unregister each one.
/// - `callback_data` is pinned for the whole lifetime of this value, and its address is the
///   `refcon` that HIServices passes back to the `AXObserverCallback`. It is therefore aliased by
///   foreign code from the first successful [`AXObserver::subscribe`] until `Drop` unregisters the
///   notifications and removes the run-loop source.
/// - `run_loop` is the `CFRunLoop` that `create` ran on — the only run loop the source was added to
///   — captured so `Drop` removes the source from that run loop rather than from whichever thread's
///   run loop happens to be current.
pub struct AXObserver<T> {
    inner: AXObserverRef,
    ax_ref: AXUIElementRef,
    run_loop: CFRunLoopRef,
    subscriptions: Vec<CFString>,
    callback_data: Pin<Box<T>>,
}

// SAFETY: PARTIAL — see docs/13-unsafe-review.md #2.
//
// Proposition asserted: `AXObserver<T>` may be moved to another thread when `T` can.
//
// What is proved:
// - `AXObserverRef` and `AXUIElementRef` are CFTypes. `CFRetain`/`CFRelease` are documented as
//   thread-safe (Apple, "Memory Management Programming Guide for Core Foundation"), so transferring
//   the pointers between threads is not itself a data race, and this value owns a reference on each
//   of them, so neither can be freed underneath it.
// - `T: Send` is now required, so the impl no longer promotes an arbitrary `!Send` payload.
// - `Drop` removes the run-loop source from the *captured* `run_loop`, not from
//   `CFRunLoopGetCurrent()`, so an off-thread drop detaches the source that was actually attached.
//   It also unregisters every notification before `callback_data` is deallocated.
//
// What remains unproved: `CFRunLoopRemoveSource` from a thread other than the run loop's owner
// is thread-safe per CFRunLoop's documented thread-safety, but a notification already *in
// flight* on the run-loop thread could still be executing `application_ax_callback` with the
// old `refcon` while the drop proceeds. Today every create/drop pair runs on the tao main thread
// (`PlatformBoundEvent::Initialize`, `reregister_all`, and the `NSWorkspace` notification
// selectors), which serialises them against the callbacks; that is a project invariant nothing
// enforces. `Sync` is no longer asserted — nothing shares an `&AXObserver` across threads.
unsafe impl<T: Send> Send for AXObserver<T> {}

impl<T> AXObserver<T> {
    /// Create an observer for `pid` and attach its run-loop source to the *current* run loop.
    ///
    /// # Safety
    ///
    /// The caller must ensure all of the following.
    ///
    /// 1. `ax_ref` is a live, non-null `AXUIElementRef` *at the moment of this call*. It is
    ///    retained here and released in `Drop`, so the caller does not have to keep it alive
    ///    afterwards.
    /// 2. `callback` is a function whose ABI is `AXObserverCallback` and which is prepared to be
    ///    invoked with `refcon == &*callback_data` — that is, it must reinterpret `refcon` as `*mut
    ///    T` and nothing else — for as long as the returned value lives.
    /// 3. `callback` runs on the thread whose run loop is current at this call.
    /// 4. The process is AX-trusted (`AXIsProcessTrusted`), otherwise `AXObserverCreate` fails and
    ///    this returns `Err` — that is an error, not UB, but callers rely on it.
    ///
    /// If those hold, the returned value owns the `+1` reference produced by `AXObserverCreate`,
    /// one reference on `ax_ref`, and a pinned `T` whose address is stable for the value's
    /// lifetime. All three are released in `Drop`.
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
        let run_loop = CFRunLoopGetCurrent();
        tracing::trace!(pid, on_main = (run_loop == CFRunLoopGetMain()), "AX observer created");
        CFRunLoopAddSource(run_loop, AXObserverGetRunLoopSource(observer), kCFRunLoopDefaultMode);

        // SAFETY:
        // Operation: the `CFRetain` FFI call.
        // Contract (Apple, "Memory Management Programming Guide for Core Foundation"):
        // `CFRetain(cf)` requires a live, non-null CFType and returns it with the count raised
        // by one; it is documented thread-safe.
        // Evidence: `ax_ref`'s liveness at this point is this function's `# Safety` clause 1.
        // Postcondition: this value owns one reference on `ax_ref`, balanced in `Drop`. That is
        // what makes the unretained-looking `ax_ref` field sound on its own, instead of relying
        // on `WindowServerInner::register` leaking the element forever
        // ("soundness by leak", docs/13-unsafe-review.md #6).
        CFRetain(ax_ref.cast());

        Ok(Self {
            inner: observer,
            ax_ref,
            run_loop,
            subscriptions: Vec::new(),
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

        if err == kAXErrorSuccess {
            // Recorded so `Drop` can unregister it. Without this the registration outlived the
            // observer and kept the `refcon` reachable.
            self.subscriptions.push(name);
            Ok(())
        } else {
            Err(err)
        }
    }
}

impl<T> Drop for AXObserver<T> {
    fn drop(&mut self) {
        // SAFETY:
        // Operations, in order: `AXObserverRemoveNotification` once per successful
        // subscription, `AXObserverGetRunLoopSource`, `CFRunLoopRemoveSource`, and two
        // `CFRelease` calls.
        //
        // Contracts:
        // - `AXObserverRemoveNotification(observer, element, notification)` (HIServices) requires a live
        //   observer, a live element and a live notification name; removing a notification that is not
        //   registered returns `kAXErrorNotificationNotRegistered`, which is an error code, not UB.
        // - `CFRunLoopRemoveSource(rl, source, mode)` (Apple CFRunLoop reference) requires non-null
        //   arguments; removing a source that is not attached is a documented no-op.
        // - `CFRelease(cf)` requires a live, non-null CFType this frame owns a reference on, and is
        //   documented thread-safe.
        //
        // Evidence:
        // - `self.inner` and `self.ax_ref` are live until this body finishes (type INVARIANT: this value
        //   owns one reference on each, taken in `create`), so every call above has live arguments and the
        //   two releases are balanced, not over-releases.
        // - `self.subscriptions` contains exactly the names `subscribe` registered successfully, each a
        //   `CFString` owned by this value.
        // - `self.run_loop` is the run loop `create` attached the source to, captured there rather than
        //   re-derived from `CFRunLoopGetCurrent()`, which is thread-local and would name the wrong run
        //   loop on an off-thread drop.
        //
        // Purpose: unregistering the notifications and removing the run-loop source is what stops
        // foreign code from dereferencing the `refcon` after `self.callback_data` is deallocated
        // immediately after this body.
        //
        // Residual risk — see docs/13-unsafe-review.md #2: a notification already in flight on
        // the run-loop thread can still be inside `application_ax_callback` while an off-thread
        // drop runs. Every create/drop pair in this crate happens on the tao main thread, which
        // serialises them; that is a project invariant nothing enforces.
        unsafe {
            for notification in &self.subscriptions {
                AXObserverRemoveNotification(self.inner, self.ax_ref, notification.as_concrete_TypeRef());
            }

            CFRunLoopRemoveSource(
                self.run_loop,
                AXObserverGetRunLoopSource(self.inner),
                kCFRunLoopDefaultMode,
            );

            CFRelease(self.inner.cast());
            CFRelease(self.ax_ref.cast());
        }
    }
}
