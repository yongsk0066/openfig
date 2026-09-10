pub mod notification_center;
mod nsarray;
mod nsstring;

use std::ops::Deref;

use cocoa::base::nil;
pub use notification_center::NotificationCenter;
pub use nsarray::NSArrayRef;
pub use nsstring::NSStringRef;
use objc::rc::StrongPtr;
use objc::runtime::Object;

/// An owning Objective-C object pointer.
///
/// # Invariants
///
/// The wrapped `StrongPtr` owns exactly one retain, which it releases on drop.
#[repr(transparent)]
#[derive(Clone)]
pub struct Id(objc::rc::StrongPtr);

impl Id {
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a live Objective-C object **that already carries a
    /// `+1` retain which this value takes over** — objc 0.2 documents `StrongPtr::new` as
    /// "constructs a `StrongPtr` to a newly created object that already has a +1 retain count.
    /// This will not retain the object." Passing a borrowed (`+0`) pointer therefore produces
    /// an over-release when the `Id` drops.
    ///
    /// Note that `Id` derives `Clone`, and `StrongPtr::clone` retains, so cloning is balanced.
    pub unsafe fn new(ptr: *mut Object) -> Self {
        // SAFETY:
        // Operation: `StrongPtr::new(ptr)`.
        // Contract (objc 0.2): "unsafe because the caller must ensure the given object pointer
        // is valid", plus the +1-ownership-transfer rule quoted above.
        // Evidence: both are this function's own `# Safety` preconditions, restated verbatim;
        // nothing between accepting them and the call can invalidate them.
        // Postcondition: the returned `Id` is the sole owner of that retain and will release it
        // on drop.
        Self(StrongPtr::new(ptr))
    }
}

impl std::ops::Deref for Id {
    type Target = StrongPtr;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
