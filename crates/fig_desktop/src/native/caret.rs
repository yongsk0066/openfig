use std::time::Duration;

use fig_util::terminal::CompletionAnchor;
use macos_utils::query_budget::QueryBudget;
use macos_utils::window_server::UIElement;

use super::{
    AnchorRect,
    CaretRequest as Request,
    FocusTarget,
};

#[derive(Default)]
pub(super) struct NativeCaret {
    target: Option<FocusTarget>,
    tree: Option<Vec<UIElement>>,
}
impl NativeCaret {
    pub fn query(&mut self, request: Request) -> Option<AnchorRect> {
        if request.anchor == CompletionAnchor::InputMethod {
            return None;
        }
        if self.target != Some(request.target) {
            self.tree = None;
            self.target = Some(request.target);
        }
        // One cooperative budget covers cache lookup, traversal, geometry and identity checks.
        // AX timeout is advisory: a stuck OS call cannot be preempted safely by Rust.
        let mut budget = QueryBudget::new(Duration::from_millis(100), 512);
        let app = UIElement::application(request.target.pid);
        let window = app.query(&mut budget, || app.focused_window().ok())?;
        // SAFETY: worker-owned AX reference; the private window-ID SPI contract remains.
        if window.query(&mut budget, || unsafe { window.get_window_id().ok() })? != request.target.window_id {
            return None;
        }
        let rect = if request.anchor == CompletionAnchor::Xterm {
            self.tree = self
                .tree
                .as_ref()
                .and_then(|cached| {
                    cached.iter().fold(None::<Vec<UIElement>>, |accum, item| match accum {
                        Some(mut path) => {
                            path.push(item.clone());
                            Some(path)
                        },
                        None => item.find_x_term_caret_tree(&mut budget),
                    })
                })
                .or_else(|| window.find_x_term_caret_tree(&mut budget));
            let leaf = self.tree.as_ref()?.first()?;
            let rect = leaf.query(&mut budget, || leaf.frame().ok())?;
            AnchorRect {
                x: rect.origin.x,
                y: rect.origin.y,
                width: rect.size.width,
                height: rect.size.height,
            }
        } else {
            let caret = macos_utils::caret_position::get_caret_position(request.target.pid, true, &mut budget);
            if !caret.valid {
                return None;
            }
            AnchorRect {
                x: caret.x,
                y: caret.y,
                width: 10.0,
                height: caret.height,
            }
        };
        // Reject an app-local window switch that occurred during the potentially slow query.
        let current = app.query(&mut budget, || app.focused_window().ok())?;
        // SAFETY: current is a live worker-owned reference; inherit the same SPI contract.
        (current.query(&mut budget, || unsafe { current.get_window_id().ok() })? == request.target.window_id)
            .then_some(rect)
    }
}
