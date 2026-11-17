mod common;

use common::*;

#[test]
fn local_state_all() -> Result<()> {
    let mut command = cli();
    command
        .args(["_", "local-state", "all", "-f", "json"])
        .assert()
        .stdout(is_json())
        .success();
    assert!(
        command
            .home()
            .join("Library/Application Support")
            .join(fig_util::consts::DATA_DIR_NAME)
            .join("data.sqlite3")
            .is_file()
    );
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
fn hostname() -> Result<()> {
    cli().args(["_", "hostname"]).assert().success();
    Ok(())
}

#[test]
fn socket_dir() -> Result<()> {
    let mut command = cli();
    let expected = command.home().join(fig_util::consts::RUNTIME_DIR_NAME);
    command
        .args(["_", "sockets-dir"])
        .assert()
        .stdout(format!("{}\n", expected.display()))
        .success();
    Ok(())
}

#[test]
fn uuidgen() -> Result<()> {
    cli().args(["_", "uuidgen"]).assert().success();
    Ok(())
}
