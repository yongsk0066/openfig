use std::process::Command;

use assert_cmd::prelude::*;
use paste::paste;

macro_rules! init_test {
    ($shell:expr, $stage:expr, $file:expr) => {
        paste! {
            fn [<init_output_ $shell _ $stage _ $file>]() -> Result<String, Box<dyn std::error::Error>> {
                let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("fig_cli"));
                cmd.arg("init").arg($shell).arg($stage).arg("--rcfile").arg($file);
                // zsh's `post` names `hook.sock` under `$TMPDIR`, which differs between machines.
                cmd.env("TMPDIR", "/fig-init-snapshot");
                let out = cmd.assert().success().get_output().stdout.clone();
                Ok(String::from_utf8(out)?)
            }

            #[test]
            fn [<init_snapshot_ $shell _ $stage _ $file>]() -> Result<(), Box<dyn std::error::Error>> {
                let init = [<init_output_ $shell _ $stage _ $file>]()?;

                insta::assert_snapshot!(init);

                Ok(())
            }
        }
    };
}

// zsh
init_test!("zsh", "pre", "zshrc");
init_test!("zsh", "pre", "zprofile");
init_test!("zsh", "post", "zshrc");
init_test!("zsh", "post", "zprofile");

/// bash support ended, but the rc blocks it installed still run `init bash pre|post` in every new
/// bash until they are uninstalled: that must print nothing and succeed, with or without
/// `--rcfile`.
#[test]
fn init_bash_is_a_silent_no_op() {
    for args in [
        &["init", "bash", "pre", "--rcfile", "bashrc"][..],
        &["init", "bash", "post", "--rcfile", "bashrc"],
        &["init", "bash", "pre", "--rcfile", "profile"],
        &["init", "bash", "post", "--rcfile", "profile"],
        &["init", "bash", "post"],
    ] {
        let output = Command::new(assert_cmd::cargo::cargo_bin!("fig_cli"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    }
}
