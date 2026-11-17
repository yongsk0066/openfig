//! Latest-only caret queries isolated from the desktop process.
use fig_desktop::native::{
    self,
    Reply,
};
pub use fig_desktop::native::{
    AnchorRect,
    CaretRequest as Request,
};

use super::focus_validation::FocusTarget;

#[derive(Debug)]
pub struct CaretWorker {
    sender: tokio::sync::watch::Sender<Option<Request>>,
}
impl CaretWorker {
    pub fn start(publish: impl Fn(Request, Option<AnchorRect>) + Send + 'static) -> std::io::Result<Self> {
        let sender = native::start_latest(native::Request::Caret, move |request, reply| {
            if let Reply::Caret(rect) = reply {
                publish(request, rect);
            }
        })?;
        Ok(Self { sender })
    }

    pub fn request(&self, request: Request) {
        self.sender.send_replace(Some(request));
    }

    pub fn cancel(&self) {
        self.sender.send_replace(None);
    }
}
pub fn accepts(current: FocusTarget, sequence: u64, result: Request) -> bool {
    current == result.target && sequence == result.sequence
}
#[cfg(test)]
mod tests {
    use fig_util::terminal::CompletionAnchor;

    use super::*;
    #[test]
    fn late_results_cannot_move_a_new_input_or_reused_window() {
        let target = FocusTarget {
            pid: 7,
            window_id: 9,
            generation: 1,
        };
        let request = Request {
            target,
            sequence: 4,
            anchor: CompletionAnchor::TextRange,
        };
        assert!(accepts(target, 4, request));
        assert!(!accepts(target, 5, request));
        assert!(!accepts(
            FocusTarget {
                generation: 2,
                ..target
            },
            4,
            request
        ));
        assert!(!accepts(FocusTarget { pid: 8, ..target }, 4, request));
    }
}
