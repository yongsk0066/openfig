//! Private inherited-pipe protocol. No listener, executable paths or commands come from clients.
mod caret;
mod client;
pub mod frame_watch;
mod icons;
mod server;

use std::path::PathBuf;

pub use client::{
    Process,
    start_latest,
};
use core_graphics::geometry::{
    CGPoint,
    CGRect,
    CGSize,
};
use fig_util::terminal::CompletionAnchor;
pub use icons::Icons;
use macos_utils::window_server::CGWindowInfo;
use serde::{
    Deserialize,
    Serialize,
};
pub use server::serve;

pub const MAX_FRAME: usize = 1024 * 1024;
pub const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
pub const ICON_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusTarget {
    pub pid: i32,
    pub window_id: u32,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretRequest {
    pub target: FocusTarget,
    pub sequence: u64,
    pub anchor: CompletionAnchor,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AnchorRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl AnchorRect {
    fn valid(&self) -> bool {
        [self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite())
            && self.width >= 0.0
            && self.height >= 0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Icon {
    Named(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Activation(i32),
    Focus(FocusTarget),
    TextFocus(FocusTarget),
    Frame(FocusTarget),
    Caret(CaretRequest),
    Icon(Icon),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Frame {
    pub target: FocusTarget,
    pub rect: AnchorRect,
    pub level: i64,
}
impl Frame {
    pub fn into_window(self) -> CGWindowInfo {
        CGWindowInfo {
            window_id: self.target.window_id,
            owner_pid: self.target.pid as u64,
            level: self.level,
            bounds: CGRect::new(
                &CGPoint::new(self.rect.x, self.rect.y),
                &CGSize::new(self.rect.width, self.rect.height),
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Reply {
    Activation {
        window: Option<u32>,
        fullscreen: Option<bool>,
    },
    Focus(Option<u32>),
    Frame(Option<Frame>),
    Caret(Option<AnchorRect>),
    /// Base64 keeps a PNG response bounded without a JSON integer per byte.
    Icon(Option<String>),
}
impl Reply {
    fn matches(&self, request: &Request) -> bool {
        match (request, self) {
            (Request::Activation(_), Self::Activation { .. }) => true,
            (Request::Focus(_) | Request::TextFocus(_), Self::Focus(_)) => true,
            (Request::Frame(target), Self::Frame(frame)) => {
                frame.as_ref().is_none_or(|f| f.target == *target && f.rect.valid())
            },
            (Request::Caret(_), Self::Caret(rect)) => rect.as_ref().is_none_or(AnchorRect::valid),
            (Request::Icon(_), Self::Icon(_)) => true,
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_ms: Option<f64>,
    id: u64,
    value: T,
}
