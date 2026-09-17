mod common;

use common::*;

#[test]
fn local_state_all() -> Result<()> {
    cli()
        .args(["_", "local-state", "all", "-f", "json"])
        .assert()
        .stdout(is_json())
        .success();
    Ok(())
}

#[test]
fn local_state_get() -> Result<()> {
    cli()
        .args(["_", "local-state", "test-value", "-f", "json"])
        .assert()
        .stdout(is_json())
        .success();
    Ok(())
}

#[test]
fn get_shell() -> Result<()> {
    cli().args(["_", "get-shell"]).assert().success();
    Ok(())
}

#[test]
fn hostname() -> Result<()> {
    cli().args(["_", "hostname"]).assert().success();
    Ok(())
}

#[test]
fn socket_dir() -> Result<()> {
    cli()
        .args(["_", "sockets-dir"])
        .assert()
        .stdout(format!("{}\n", fig_util::directories::sockets_dir_utf8().unwrap()))
        .success();
    Ok(())
}

#[test]
fn uuidgen() -> Result<()> {
    cli().args(["_", "uuidgen"]).assert().success();
    Ok(())
}
