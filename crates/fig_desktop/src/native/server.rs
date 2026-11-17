use std::io::{
    self,
    Read,
    Write,
};
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use macos_utils::query_budget::QueryBudget;
use macos_utils::window_server::UIElement;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSApplication,
    NSApplicationActivationPolicy,
};

use super::caret::NativeCaret;
use super::{
    AnchorRect,
    Envelope,
    Frame,
    Icon,
    MAX_FRAME,
    Reply,
    Request,
};

/// Runs on the helper's main thread, never the desktop's AppKit thread.
pub fn serve() -> io::Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| io::Error::other("helper requires its main thread"))?;
    UIElement::set_global_messaging_timeout(0.1);
    let mut caret = NativeCaret::default();
    let mut icons_initialized = false;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut header = [0; 4];
        match input.read_exact(&mut header) {
            Ok(()) => {},
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let size = u32::from_be_bytes(header) as usize;
        if size > MAX_FRAME {
            return Err(io::Error::other("native request too large"));
        }
        let mut bytes = vec![0; size];
        input.read_exact(&mut bytes)?;
        let request: Envelope<Request> = serde_json::from_slice(&bytes)?;
        // A panic/crash terminates only this process. Parent owns recovery, including stuck FFI.
        let started = std::time::Instant::now();
        let value = autoreleasepool(|_| match request.value {
            Request::Activation(pid) => {
                let window = UIElement::application(pid).focused_window().ok();
                // SAFETY: helper-owned AX reference, subject to the documented window-ID SPI
                // contract.
                Reply::Activation {
                    window: window
                        .as_ref()
                        .and_then(|window| unsafe { window.get_window_id().ok() }),
                    fullscreen: window.and_then(|window| window.is_fullscreen().ok()),
                }
            },
            Request::Focus(target) => Reply::Focus(UIElement::application(target.pid).focused_window().ok().and_then(
                |window| {
                    // SAFETY: the helper owns this live AX reference; private window-ID SPI
                    // contract.
                    unsafe { window.get_window_id().ok() }
                },
            )),
            Request::TextFocus(target) => Reply::Focus(
                UIElement::application(target.pid)
                    .focused_text_area_window_id(&mut QueryBudget::new(Duration::from_millis(100), 8)),
            ),
            Request::Frame(target) => Reply::Frame(
                UIElement::window_info_for_id(target.window_id)
                    .filter(|frame| frame.owner_pid == target.pid as u64)
                    .map(|frame| Frame {
                        target,
                        level: frame.level,
                        rect: AnchorRect {
                            x: frame.bounds.origin.x,
                            y: frame.bounds.origin.y,
                            width: frame.bounds.size.width,
                            height: frame.bounds.size.height,
                        },
                    }),
            ),
            Request::Caret(request) => Reply::Caret(caret.query(request)),
            Request::Icon(icon) => {
                if !icons_initialized {
                    NSApplication::sharedApplication(mtm)
                        .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
                    icons_initialized = true;
                }
                Reply::Icon(render_icon(mtm, icon).map(|bytes| STANDARD.encode(bytes)))
            },
        });
        let bytes = serde_json::to_vec(&Envelope {
            id: request.id,
            value,
            work_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
        })?;
        if bytes.len() > MAX_FRAME {
            return Err(io::Error::other("native response too large"));
        }
        output.write_all(&(bytes.len() as u32).to_be_bytes())?;
        output.write_all(&bytes)?;
        output.flush()?;
    }
}

fn render_icon(mtm: MainThreadMarker, icon: Icon) -> Option<Vec<u8>> {
    use macos_utils::image::{
        png_for_name,
        png_for_path,
    };
    match icon {
        Icon::Named(name) => png_for_name(mtm, &name),
        Icon::Path(path) => png_for_path(mtm, &path)
            .or_else(|| {
                path.to_str()?
                    .ends_with('/')
                    .then(|| png_for_path(mtm, Path::new("/bin")))
                    .flatten()
            })
            .or_else(|| png_for_name(mtm, path.extension()?.to_str()?))
            .or_else(|| png_for_name(mtm, "file")),
    }
}
