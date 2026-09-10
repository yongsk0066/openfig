#![allow(dead_code)]

use std::ffi::c_void;
use std::marker::PhantomData;

use appkit_nsworkspace_bindings::NSArray as AppkitNSArray;
use cocoa::base::id as RawId;
use cocoa::foundation::{
    NSArray as CocoaNSArray,
    NSUInteger,
};
use core_foundation::array::CFArrayRef;

use super::{
    Id,
    IdRef,
};

pub struct NSArray<T: 'static> {
    inner: Id,
    phantom: PhantomData<T>,
}

impl<T: 'static> NSArray<T> {
    pub fn iter(self) -> NSArrayIter<T> {
        let count = self.len();
        NSArrayIter {
            inner: self.inner,
            count,
            index: 0,
            phantom: PhantomData,
        }
    }

    pub fn len(&self) -> u64 {
        // SAFETY:
        // Operation: `objc_msgSend` for `-[NSArray count]` (cocoa-foundation's
        // `NSArray::count` on `id`).
        // Contract (ObjC ABI): the receiver must be a live object that responds to `count`
        // with an `NSUInteger` return.
        // Evidence: `self.inner` is an `Id`, whose type invariant is that it owns a live retain
        // (see `Id::new`), and `NSArray` is documented to respond to `-count`.
        unsafe { self.inner.count() }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> std::ops::Deref for NSArray<T> {
    type Target = Id;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> From<AppkitNSArray> for NSArray<T> {
    fn from(a: AppkitNSArray) -> Self {
        // SAFETY: UNPROVEN — see docs/13-unsafe-review.md #15.
        // Operation: `Id::new(a.0)`.
        // Contract (see `Id::new`): `a.0` must be a live object carrying a `+1` retain that
        // this `Id` takes over.
        // Evidence available: `appkit_nsworkspace_bindings::NSArray` is a bindgen newtype over
        // an `id` with no `Drop` impl, so nothing else will release it.
        // Missing premise: nothing establishes that the pointer is `+1` rather than `+0`.
        // bindgen-generated Objective-C accessors normally return autoreleased (`+0`) objects,
        // in which case this conversion over-releases on drop. This is a *safe* `From` impl, so
        // safe code could reach that over-release — the only reason it is not a live
        // soundness hole is that nothing in the workspace constructs an `NSArray<T>` this way
        // (the whole owning `NSArray` type is dead code behind `#![allow(dead_code)]`).
        Self {
            inner: unsafe { Id::new(a.0) },
            phantom: PhantomData,
        }
    }
}

pub struct NSArrayIter<T: 'static> {
    inner: Id,
    count: NSUInteger,
    index: NSUInteger,
    phantom: PhantomData<T>,
}

impl<T: 'static> Iterator for NSArrayIter<T> {
    type Item = IdRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            None
        } else {
            // SAFETY:
            // Operation: `objc_msgSend` for `-[NSArray objectAtIndex:]`.
            // Contract (Apple Foundation): the receiver must be a live `NSArray` and `index`
            // must be `< count`, otherwise the method raises `NSRangeException` (an ObjC
            // exception crossing a Rust frame, i.e. UB, not a Rust panic).
            // Evidence:
            // - Liveness: `self.inner` is an owning `Id` (type invariant of `Id`).
            // - `self.index < self.count`: LOCAL FACT from the branch condition immediately above; `count` was
            //   captured in `NSArray::iter` and both fields are private, so no other code can change them
            //   between the check and the call.
            // UNPROVEN-BUT-LIKELY: `count` is captured once at iterator construction. If the
            // array were mutable and shrank during iteration, the bound would be stale. The
            // premise that it cannot is "the caller only wraps immutable arrays", which the
            // type does not enforce.
            let item = unsafe { self.inner.objectAtIndex(self.index) };
            self.index += 1;
            // SAFETY:
            // Operation: `IdRef::new(item)`.
            // Contract: null, or live for the lifetime of the returned `IdRef`.
            // Evidence: `-objectAtIndex:` returns a `+0` element owned by the array; the array
            // is kept alive by `self.inner` for as long as the iterator lives. The `IdRef` has
            // no lifetime parameter, so this is only sound while the caller keeps the iterator
            // alive — see docs/13-unsafe-review.md #13.
            Some(unsafe { IdRef::new(item) })
        }
    }
}

impl<T> IntoIterator for NSArray<T> {
    type IntoIter = NSArrayIter<T>;
    type Item = IdRef;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct NSArrayRef<T: 'static> {
    inner: IdRef,
    phantom: PhantomData<T>,
}

impl<T: 'static> NSArrayRef<T> {
    /// # Safety
    ///
    /// The caller must ensure that `inner` is `nil` or a live `NSArray` (or a toll-free-bridged
    /// `CFArrayRef`) that stays alive for the whole lifetime of the returned value and of every
    /// `IdRef` its iterator yields. This type is non-owning: it never retains.
    pub unsafe fn new(inner: *const objc::runtime::Object) -> Self {
        // SAFETY:
        // Operation: `IdRef::new(inner)`, which only stores the pointer.
        // Contract: null or live for the borrow's lifetime.
        // Evidence: this function's own `# Safety` precondition, restated unchanged.
        Self {
            inner: IdRef::new(inner),
            phantom: PhantomData,
        }
    }

    pub fn iter(self) -> NSArrayRefIter<T> {
        let count = self.len();
        NSArrayRefIter {
            inner: self.inner,
            count,
            index: 0,
            phantom: PhantomData,
        }
    }

    pub fn len(&self) -> u64 {
        // SAFETY:
        // Operations: the `*const Object -> *mut Object` cast and `objc_msgSend` for
        // `-[NSArray count]`.
        // Contract (ObjC ABI): the receiver must be a live object responding to `count`.
        // Evidence: liveness is the `# Safety` obligation recorded on `NSArrayRef::new` /
        // `IdRef::new` and inherited by whichever `From` impl produced this value.
        // Note: a `nil` receiver would return 0 under the ObjC nil-messaging rule rather than
        // crash, but `is_nil` is not checked here — `len()` on a nil array silently yields 0.
        unsafe { (*self.inner as RawId).count() }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> std::ops::Deref for NSArrayRef<T> {
    type Target = IdRef;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// # Invariants
///
/// Non-owning: the `CFArrayRef`'s retain count is untouched, so the caller must keep the array
/// alive for as long as the `NSArrayRef` and anything derived from it is used. This is a *safe*
/// `From` impl carrying an unchecked memory-safety obligation; see docs/13-unsafe-review.md #10.
impl<T> From<CFArrayRef> for NSArrayRef<T> {
    fn from(arr_ref: CFArrayRef) -> Self {
        // SAFETY:
        // Operations: the `CFArrayRef -> *mut c_void -> id` pointer casts and `IdRef::new`.
        // Contract of `IdRef::new`: null or live for the borrow's lifetime.
        // Contract behind the cast: `CFArray` and `NSArray` are toll-free bridged (Apple,
        // "Toll-Free Bridged Types"), so a `CFArrayRef` may be messaged as an `NSArray`.
        // Evidence: liveness is the caller obligation in the `# Invariants` section above. The
        // one live caller, `UIElement::attribute_list`, passes a `+1` array from
        // `AXUIElementCopyAttributeNames` that it never releases, so the array outlives the
        // borrow — soundness by leak.
        Self {
            inner: unsafe { IdRef::new(arr_ref as *mut c_void as RawId) },
            phantom: PhantomData,
        }
    }
}

impl<T> From<AppkitNSArray> for NSArrayRef<T> {
    fn from(a: AppkitNSArray) -> Self {
        // SAFETY:
        // Operation: `IdRef::new(a.0)`, which only stores the pointer (no retain, no release).
        // Contract: null or live for the borrow's lifetime.
        // Evidence: `AppkitNSArray` is a bindgen newtype over an `id` with no `Drop`, so this
        // conversion changes no retain count. Unlike the owning `From<AppkitNSArray> for
        // NSArray<T>` above, a `+0` input is fine here.
        // UNPROVEN: liveness is inherited from the producer of the `AppkitNSArray` and cannot
        // be discharged locally. Currently dead code.
        Self {
            inner: unsafe { IdRef::new(a.0) },
            phantom: PhantomData,
        }
    }
}

pub struct NSArrayRefIter<T: 'static> {
    inner: IdRef,
    count: NSUInteger,
    index: NSUInteger,
    phantom: PhantomData<T>,
}

impl<T: 'static> Iterator for NSArrayRefIter<T> {
    type Item = IdRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            None
        } else {
            // SAFETY:
            // Operations: the `*const Object -> id` cast and `objc_msgSend` for
            // `-[NSArray objectAtIndex:]`.
            // Contract (Apple Foundation): live `NSArray` receiver and `index < count`;
            // otherwise `NSRangeException` unwinds through Rust frames.
            // Evidence:
            // - Liveness: the `# Safety` obligation of `NSArrayRef::new` / the `From` impls, discharged by the
            //   caller. For the one live path (`UIElement::attribute_list`) the array is leaked, hence alive.
            // - `self.index < self.count`: LOCAL FACT from the branch above; `count` was read in
            //   `NSArrayRef::iter` and both fields are private and only written here.
            let item = unsafe { (*self.inner as RawId).objectAtIndex(self.index) };
            self.index += 1;
            // SAFETY:
            // Operation: `IdRef::new(item)`.
            // Contract: null or live for the borrow's lifetime.
            // Evidence: elements of an `NSArray` are owned by the array and live at least as
            // long as it; the array outlives this iterator by the premise above.
            Some(unsafe { IdRef::new(item) })
        }
    }
}

impl<T> IntoIterator for NSArrayRef<T> {
    type IntoIter = NSArrayRefIter<T>;
    type Item = IdRef;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
