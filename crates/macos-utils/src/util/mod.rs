pub mod notification_center;
mod nsstring;

use std::ops::Deref;

use cocoa::base::nil;
pub use notification_center::NotificationCenter;
pub use nsstring::NSStringRef;
use objc::runtime::Object;

/// A non-owning reference to an Objective-C object.
#[repr(transparent)]
pub struct IdRef(*const Object);

impl IdRef {
    /// # Safety
    ///
    /// The caller must ensure that `inner` is either `nil` or a pointer to a live
    /// Objective-C object, and that the object stays alive for the entire lifetime of the
    /// returned `IdRef` and of every value derived from it (`NSStringRef`, `NSArrayRef`, and
    /// any `&str` handed out by them).
    ///
    /// This type does **not** retain and has **no lifetime parameter**, so the second clause
    /// is entirely on the caller; the borrow checker cannot help. `is_nil` is the only
    /// dynamically checked part of the contract. See docs/13-unsafe-review.md #13.
    pub unsafe fn new(inner: *const Object) -> IdRef {
        // SAFETY:
        // Operation: none. This constructor performs no unsafe operation at all — it only
        // stores the pointer. It is `unsafe fn` because the *obligation* it records above is
        // discharged later, by `NSStringRef::as_str` and `NSArrayRef::len`/`iter`, which do
        // dereference. Marking the constructor is the standard way to place the obligation at
        // the point where the pointer's provenance is known.
        IdRef(inner)
    }

    pub fn is_nil(&self) -> bool {
        self.0 == nil
    }
}

impl Deref for IdRef {
    type Target = *const Object;

    fn deref(&self) -> &*const Object {
        &self.0
    }
}
