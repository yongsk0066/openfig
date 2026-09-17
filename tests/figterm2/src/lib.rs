pub mod latency;
mod shell;
pub use shell::{
    Desktop,
    Echo,
    EditBufferEvent,
    KeystrokeTiming,
    Shell,
    cli_binary,
    pty_binary,
};
