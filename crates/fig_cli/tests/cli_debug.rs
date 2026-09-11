mod common;

use common::*;

#[test]
fn debug_root() -> Result<()> {
    cli().arg("debug").assert().code(predicate::eq(2));
    Ok(())
}

/// `q debug verify-codesign` is gone; clap rejects an unknown subcommand with 2.
#[test]
fn debug_verify_codesign_is_gone() -> Result<()> {
    cli().args(["debug", "verify-codesign"]).assert().code(predicate::eq(2));
    Ok(())
}
