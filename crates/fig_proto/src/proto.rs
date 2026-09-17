#![allow(clippy::all)]

/// Fig Common Protocol Buffers
///
/// These are shared definition used in the other
/// protocol buffer definition.
pub(crate) mod fig_common {
    include!(concat!(env!("OUT_DIR"), "/fig_common.rs"));
}

/// Fig.js Protocol Buffers
pub(crate) mod fig {
    pub use super::fig_common::*;
    include!(concat!(env!("OUT_DIR"), "/fig.rs"));
}

/// Local Protocol Buffers
pub(crate) mod local {
    pub use super::fig_common::*;
    include!(concat!(env!("OUT_DIR"), "/local.rs"));
}

/// What the desktop asks of a shell session
pub(crate) mod shell {
    include!(concat!(env!("OUT_DIR"), "/shell.rs"));
}
