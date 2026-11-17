use std::ptr::NonNull;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{
    NSObjectProtocol,
    ProtocolObject,
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

#[derive(Clone)]
enum Inner {
    NotificationCenter(Retained<NSNotificationCenter>),
    DistributedNotificationCenter(Retained<NSDistributedNotificationCenter>),
}

pub struct NotificationCenter {
    inner: Inner,
}

/// Owns a block registration; dropping it unregisters before releasing the token.
#[must_use = "retain the subscription for as long as notifications are needed"]
pub struct NotificationSubscription {
    center: Inner,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    _main_thread: MainThreadMarker,
}

impl std::fmt::Debug for NotificationSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationSubscription").finish_non_exhaustive()
    }
}

impl Drop for NotificationSubscription {
    fn drop(&mut self) {
        // SAFETY: both center and observer token remain retained during removal. The block
        // captures only Send + Sync data; an already-running callback cannot borrow this guard.
        unsafe {
            match &self.center {
                Inner::NotificationCenter(center) => center.removeObserver((*self.token).as_ref()),
                Inner::DistributedNotificationCenter(center) => center.removeObserver((*self.token).as_ref()),
            }
        }
    }
}

impl NotificationCenter {
    pub(crate) fn workspace_center() -> Self {
        Self {
            inner: Inner::NotificationCenter(NSWorkspace::sharedWorkspace().notificationCenter()),
        }
    }

    pub fn distributed_center() -> Self {
        Self {
            inner: Inner::DistributedNotificationCenter(NSDistributedNotificationCenter::defaultCenter()),
        }
    }

    pub fn subscribe<F>(
        &self,
        mtm: MainThreadMarker,
        name: &NSNotificationName,
        queue: Option<&NSOperationQueue>,
        f: F,
    ) -> NotificationSubscription
    where
        F: Fn(&NSNotification) + Send + Sync + Clone + 'static,
    {
        let block = block2::StackBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: Foundation keeps the notification alive for the duration of delivery.
            f(unsafe { notification.as_ref() });
        });
        // SAFETY: Foundation copies the block before returning. Captures are owned and safe
        // on any posting/operation-queue thread; callers only receive a callback-scoped borrow.
        let token = unsafe {
            match &self.inner {
                Inner::NotificationCenter(center) => {
                    center.addObserverForName_object_queue_usingBlock(Some(name), None, queue, &block)
                },
                Inner::DistributedNotificationCenter(center) => {
                    center.addObserverForName_object_queue_usingBlock(Some(name), None, queue, &block)
                },
            }
        };
        NotificationSubscription {
            center: self.inner.clone(),
            token,
            _main_thread: mtm,
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
