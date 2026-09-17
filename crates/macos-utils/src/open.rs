//! Handing a URL to the user's default handler via `NSWorkspace`.
//!
//! This was `fig_util::open`, and it was the only reason `fig_util` — linked by the CLI and every
//! library crate — pulled in `objc2`, `objc2-app-kit` and `objc2-foundation`.
//! Both callers (`fig_desktop_api::requests::other`, `fig_desktop::local_ipc::commands`) already
//! depend on this crate, which is where the Objective-C bridging belongs.

/// `NSWorkspace` reports only success or failure, so there is nothing else to carry.
///
/// The enum used to have an `Io(#[from] std::io::Error)` variant alongside this, left from the
/// `xdg-open` subprocess path; `open_url` cannot produce one.
#[derive(Debug, thiserror::Error)]
#[error("failed to open URL")]
pub struct OpenUrlError;

/// Open `url` with whatever application the user has registered for its scheme.
///
/// Returns as soon as `NSWorkspace` has accepted the URL; launching the handler is asynchronous
/// in the OS, so success here does not mean the application is up. There is deliberately no
/// `async` wrapper — the old `open_url_async` awaited nothing.
pub fn open_url(url: impl AsRef<str>) -> Result<(), OpenUrlError> {
    use objc2::AllocAnyThread;
    use objc2_foundation::{
        NSString,
        NSURL,
    };

    let url_nsstring = NSString::from_str(url.as_ref());
    let nsurl = NSURL::initWithString(NSURL::alloc(), &url_nsstring).ok_or(OpenUrlError)?;
    objc2_app_kit::NSWorkspace::sharedWorkspace()
        .openURL(&nsurl)
        .then_some(())
        .ok_or(OpenUrlError)
}
