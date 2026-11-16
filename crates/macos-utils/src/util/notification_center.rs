use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{
    AnyObject,
    Sel,
};
use objc2_app_kit::{
    NSRunningApplication,
    NSWorkspace,
};
use objc2_foundation::{
    NSDistributedNotificationCenter,
    NSNotification,
    NSNotificationCenter,
    NSNotificationName,
    NSOperationQueue,
    ns_string,
};

enum Inner {
    NotificationCenter(Retained<NSNotificationCenter>),
    DistributedNotificationCenter(Retained<NSDistributedNotificationCenter>),
}

pub struct NotificationCenter {
    inner: Inner,
}

impl NotificationCenter {
    fn new(center: Retained<NSNotificationCenter>) -> Self {
        Self {
            inner: Inner::NotificationCenter(center),
        }
    }

    pub(crate) fn workspace_center() -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        Self::new(center)
    }

    pub fn distributed_center() -> Self {
        let center = NSDistributedNotificationCenter::defaultCenter();
        Self {
            inner: Inner::DistributedNotificationCenter(center),
        }
    }

    /// Unregister `observer` from every notification it is registered for on this center.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `observer` is a live object. It need not currently be
    /// registered: Apple documents `-removeObserver:` as a no-op for an unregistered observer.
    ///
    /// Callers that registered with [`Self::subscribe_with_observer`] must call this *before*
    /// releasing their last strong reference to `observer`, because notification centers keep
    /// unretained (`__unsafe_unretained`) references to selector-style observers.
    pub(crate) unsafe fn remove_observer(&self, observer: &AnyObject) {
        // SAFETY:
        // Operation: `objc_msgSend` for `-[NSNotificationCenter removeObserver:]` via objc2's
        // generated (unsafe) binding.
        // Contract: live receiver, live `observer`, and — per objc2 — the argument must be a
        // valid object pointer.
        // Evidence:
        // - The receiver is a `Retained<NSNotificationCenter>` /
        //   `Retained<NSDistributedNotificationCenter>` held by `self.inner`, so it is live for
        //   `&self` (TYPE FACT of `Retained`).
        // - `observer` is a `&AnyObject`; Rust reference validity gives liveness for the call.
        // - `NSNotificationCenter` is documented thread-safe, so no synchronization obligation is
        //   left to the caller.
        match &self.inner {
            Inner::NotificationCenter(i) => i.removeObserver(observer),
            Inner::DistributedNotificationCenter(i) => i.removeObserver(observer),
        }
    }

    /// Register `observer` to receive `notification_name` by sending it `callback`.
    ///
    /// # Safety
    ///
    /// The caller must ensure all of the following, because none of it is checked by rustc —
    /// Objective-C selector dispatch is resolved at run time:
    ///
    /// 1. `observer`'s class implements `callback` with the signature
    ///    `-(void)callback:(NSNotification *)notification`. A mismatch is an ABI violation on every
    ///    delivery.
    /// 2. `observer` stays alive until [`Self::remove_observer`] is called with it on this same
    ///    center. Notification centers store selector-style observers unretained, so releasing the
    ///    observer first leaves the center with a dangling pointer and the next matching
    ///    notification is a use-after-free.
    /// 3. `observer` and `callback` tolerate being invoked on whatever thread posts the
    ///    notification. `NSWorkspace`'s center posts on the main thread;
    ///    `NSDistributedNotificationCenter` posts on the thread that is running the registered run
    ///    loop.
    pub(crate) unsafe fn subscribe_with_observer(
        &mut self,
        notification_name: &NSNotificationName,
        observer: &AnyObject,
        callback: Sel,
    ) {
        // SAFETY:
        // Operation: `objc_msgSend` for `-addObserver:selector:name:object:`.
        // Contract: live receiver and arguments, plus the three clauses above.
        // Evidence: receiver liveness from `Retained` in `self.inner`; `observer` and
        // `notification_name` are Rust references, hence live for the call; `Sel` is a
        // registered selector by construction (`sel!` interns it). Clauses 1-3 are this
        // function's own `# Safety` preconditions, restated unchanged and discharged by
        // `WindowServer::new` / `WindowServer::drop`.
        match &self.inner {
            Inner::NotificationCenter(i) => {
                i.addObserver_selector_name_object(observer, callback, Some(notification_name), None);
            },
            Inner::DistributedNotificationCenter(i) => {
                i.addObserver_selector_name_object(observer, callback, Some(notification_name), None);
            },
        }
    }

    pub fn subscribe<F>(&mut self, notification_name: &NSNotificationName, queue: Option<&NSOperationQueue>, f: F)
    where
        F: Fn(NonNull<NSNotification>) + Clone + 'static,
    {
        let block = block2::StackBlock::new(f);
        // SAFETY:
        // Operation: `objc_msgSend` for `-addObserverForName:object:queue:usingBlock:`.
        // Contract: live receiver and arguments; the block must outlive the registration, and
        // it must be callable with a `NonNull<NSNotification>` argument on the given queue.
        // Evidence:
        // - Receiver liveness from `Retained` in `self.inner`.
        // - Block lifetime: `StackBlock` lives on this stack frame, but Apple documents
        //   `-addObserverForName:object:queue:usingBlock:` as *copying* the block to the heap
        //   (`Block_copy`) before returning. block2 0.6 models this by accepting `&StackBlock` for
        //   parameters the callee copies. That copy is what makes the stack block sound here — the
        //   original comment ("addObserverForName copies block for us") is right, this spells out
        //   why it is sufficient.
        // - `F: Fn(NonNull<NSNotification>) + Clone + 'static` — `'static` rules out the block
        //   capturing anything that could dangle after this frame returns, and `Clone` is what
        //   block2 requires to produce the heap copy.
        // - Thread: the block runs on `queue` if given, else on the posting thread. The one caller
        //   (`fig_desktop`'s accessibility watcher) only sends on an `EventLoopProxy`, which is
        //   `Send`.
        // Leak note: the returned opaque observer token is discarded, so this registration can
        // never be removed. That is intentional for a process-lifetime watcher, but it means
        // `remove_observer` cannot undo it.
        unsafe {
            match &self.inner {
                Inner::NotificationCenter(i) => {
                    i.addObserverForName_object_queue_usingBlock(Some(notification_name), None, queue, &block);
                },
                Inner::DistributedNotificationCenter(i) => {
                    i.addObserverForName_object_queue_usingBlock(Some(notification_name), None, queue, &block);
                },
            }
        }
    }
}

/// Read an application notification using a checked Objective-C downcast.
pub fn get_app_from_notification(notification: &NSNotification) -> Option<Retained<NSRunningApplication>> {
    notification
        .userInfo()?
        .objectForKey(ns_string!("NSWorkspaceApplicationKey"))?
        .downcast()
        .ok()
}
