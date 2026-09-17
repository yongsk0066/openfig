mod env;
pub mod process_info;

use std::sync::Arc;

pub use env::Env;
pub use process_info::ProcessInfo;

pub trait Shim {
    /// Returns whether or not the shim is a real implementation.
    fn is_real(&self) -> bool;
}

/// The process environment, real or faked, and the process tree.
///
/// Tests cannot otherwise control the environment without `unsafe` writes, and several code paths
/// read `HOME` and friends.
#[derive(Debug, Clone)]
pub struct Context {
    env: Env,
    process_info: ProcessInfo,
}

impl Context {
    /// Returns a new [Context] with real implementations of each OS shim.
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|ctx| Self {
            env: Default::default(),
            process_info: ProcessInfo::new(ctx.clone()),
        })
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn process_info(&self) -> &ProcessInfo {
        &self.process_info
    }
}

#[derive(Default, Debug)]
pub struct ContextBuilder {
    env: Option<Env>,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds an immutable [Context] using real implementations for each field by default.
    pub fn build(self) -> Arc<Context> {
        let env = self.env.unwrap_or_default();
        Arc::new_cyclic(|ctx| Context {
            env,
            process_info: ProcessInfo::new(ctx.clone()),
        })
    }

    pub fn with_env(mut self, env: Env) -> Self {
        self.env = Some(env);
        self
    }

    pub fn with_env_var(mut self, key: &str, value: &str) -> Self {
        self.env = match self.env {
            Some(env) if !env.is_real() => {
                // SAFETY:
                // Operation: `Env::set_var`, an unsafe fn.
                // Contract (see its `# Safety`): no concurrent environment access — but only
                // for a *real* environment; the fake branch mutates a `Mutex<HashMap>` and has
                // no precondition.
                // Evidence: the `if !env.is_real()` guard on this match arm is a LOCAL FACT
                // establishing that `env`'s `Inner` is `Fake`. `Env` is not `Clone`-into-real
                // and has no interior mutability that could flip the variant, so the fact still
                // holds at the call. `std::env::set_var` is therefore not reached at all and
                // the process environment is untouched.
                // This is the case the `# Safety` section of `Env::set_var` calls out as
                // provable by the caller.
                unsafe { env.set_var(key, value) };
                Some(env)
            },
            _ => Some(Env::from_slice(&[(key, value)])),
        };
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_builder_returns_real_impls_by_default() {
        let ctx = ContextBuilder::new().build();
        assert!(ctx.env().is_real());
    }

    /// `with_env_var` on a builder that already holds a fake environment adds to it: the
    /// `!env.is_real()` arm, whose `Env::set_var` call the `SAFETY` proof in `with_env_var`
    /// covers.
    #[test]
    fn with_env_var_extends_a_fake_env() {
        let ctx = ContextBuilder::new()
            .with_env(Env::from_slice(&[("HOME", "/home/testuser")]))
            .with_env_var("hello", "world")
            .build();
        assert!(!ctx.env().is_real());
        assert_eq!(ctx.env().get("HOME").unwrap(), "/home/testuser");
        assert_eq!(ctx.env().get("hello").unwrap(), "world");
    }
}
