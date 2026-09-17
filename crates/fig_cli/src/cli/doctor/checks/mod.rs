mod bash_version;
mod zsh_hook;

pub use bash_version::BashVersionCheck;
pub use zsh_hook::{
    ZshHookCheck,
    current_shell_uses_hook,
};
