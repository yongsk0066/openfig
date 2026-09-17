pub mod golden;
pub mod latency;
mod shell;
pub use shell::{
    Desktop,
    Echo,
    EditBufferEvent,
    KeystrokeTiming,
    Refusals,
    Shell,
    cli_binary,
    pty_binary,
};
