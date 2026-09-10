use appkit_nsworkspace_bindings::NSString as AppkitNSString;
use cocoa::foundation::NSString as CocoaNSString;
use objc::runtime::Object;

use super::IdRef;

/// This is a borrowed NSString
#[repr(transparent)]
pub struct NSStringRef(IdRef);

impl NSStringRef {
    /// # Safety
    ///
    /// The caller must ensure that `inner` is either `nil` or a pointer to a live `NSString`
    /// (or a toll-free-bridged `CFStringRef`), and that the object stays alive for the whole
    /// lifetime of the returned `NSStringRef` — including across every `as_str` call and for
    /// as long as any `&str` returned by `as_str` is still in use.
    ///
    /// This type does not retain: it is a borrow with no lifetime parameter, so nothing in the
    /// type system enforces the second clause. See docs/13-unsafe-review.md #13.
    pub unsafe fn new(inner: *const Object) -> Self {
        Self(IdRef::new(inner))
    }

    /// The string's UTF-8 bytes, or `None` if the receiver is `nil`, the string has no UTF-8
    /// representation, or the bytes are not well-formed UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        if self.0.is_nil() {
            None
        } else {
            // SAFETY:
            // Operations, in order:
            // 1. `obj.UTF8String()` — `objc_msgSend` for `-[NSString UTF8String]`.
            // 2. `obj.len()` — `objc_msgSend` for `-[NSString lengthOfBytesUsingEncoding:NSUTF8StringEncoding]`
            //    (cocoa-foundation 0.2 defines `NSString::len` as exactly that).
            // 3. `std::slice::from_raw_parts(bytes as *const u8, len)`.
            //
            // (1)+(2) Contract (objc2/objc `msg_send!` and the ObjC ABI): the receiver must be
            // a live object that responds to the selector with the declared signature.
            // Evidence: `self.0` is non-null on this branch (the `is_nil` check above, and
            // `self.0` is an immutable field that nothing can change in between), and it is a
            // live `NSString` by this type's `# Safety` contract on `new`.
            //
            // (3) Contract (std `slice::from_raw_parts`): `data` must be non-null and aligned,
            // the whole `len` bytes must lie in one live allocation and be initialized, and the
            // memory must not be mutated for the returned lifetime.
            // Evidence:
            // - Non-null: checked explicitly below. Apple documents `-UTF8String` as returning a NUL-terminated
            //   UTF-8 representation, but the return is not documented as never-nil, and the std contract
            //   requires non-null *even when `len == 0`*, so the check is load-bearing rather than defensive.
            //   See docs/13-unsafe-review.md #14 — the previous code passed the pointer through unchecked.
            // - Alignment: the element type is `u8`, alignment 1, satisfied by every address.
            // - Length and initialization: `lengthOfBytesUsingEncoding:` is documented to return "the number of
            //   bytes required to store the receiver in the given encoding", excluding the terminating NUL —
            //   exactly the number of bytes `-UTF8String` writes before its NUL. DEPENDENCY LEMMA (Apple
            //   Foundation), and the premise that the two messages agree is the crux of this proof.
            // - Single allocation and no mutation: `-UTF8String` returns an autoreleased, immutable buffer
            //   owned by the string or the current autorelease pool.
            //
            // UNPROVEN (temporal) — see docs/13-unsafe-review.md #13: the returned `&str`
            // borrows `self`, but the buffer's real lifetime is bounded by the innermost
            // autorelease pool, which may drain before `self` is dropped. Every in-crate caller
            // copies the result with `to_owned()` inside the same expression, so no `&str`
            // currently outlives a pool drain, but the signature promises more than that.
            //
            // `str::from_utf8` replaces the previous `from_utf8_unchecked`: `NSString` can hold
            // unpaired surrogates, for which the UTF-8 representation is not well-formed, and
            // relying on an Apple documentation lemma to skip the Rust validity check is
            // exactly the kind of premise the review skill rejects. The check is O(len) on
            // strings that are at most a window title or an AX attribute name.
            unsafe {
                let obj = *self.0 as *mut Object;
                let bytes: *const std::os::raw::c_char = obj.UTF8String();
                if bytes.is_null() {
                    return None;
                }
                let len = obj.len();
                let bytes = std::slice::from_raw_parts(bytes as *const u8, len);
                std::str::from_utf8(bytes).ok()
            }
        }
    }
}

impl std::ops::Deref for NSStringRef {
    type Target = *const Object;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<AppkitNSString> for NSStringRef {
    fn from(s: AppkitNSString) -> Self {
        // SAFETY:
        // Operation: `IdRef::new(s.0)`.
        // Contract (see `IdRef::new`): the pointer must be null or a live object that outlives
        // the returned borrow.
        // Evidence: `appkit_nsworkspace_bindings::NSString` is a bindgen newtype over an
        // Objective-C `id` produced by the generated NSWorkspace bindings; it carries no `Drop`
        // impl, so converting it here neither releases nor transfers ownership — the borrow's
        // validity is inherited from whatever produced the `AppkitNSString`.
        // UNPROVEN: `AppkitNSString` has no documented ownership contract of its own, so
        // "outlives the borrow" cannot be discharged locally. This impl is currently dead code
        // (nothing in the workspace constructs an `NSStringRef` this way); see
        // docs/13-unsafe-review.md, adjacent smells.
        Self(unsafe { IdRef::new(s.0) })
    }
}
