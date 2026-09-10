// Mega yikes on this.
#![allow(deprecated)]
#![allow(unsafe_op_in_unsafe_fn)]

pub mod accessibility;
pub mod bundle;
pub mod caret_position;
pub mod image;
pub mod open;
pub mod os;
mod util;
pub mod window_server;

pub use open::open_url;
pub use util::NotificationCenter;
pub use window_server::{
    WindowServer,
    WindowServerEvent,
};
