use std::process::Command;

use assert_cmd::prelude::*;

macro_rules! init_test {
    ($name:ident, $shell:expr, $stage:expr, $file:expr) => {
        #[test]
        fn $name() -> Result<(), Box<dyn std::error::Error>> {
            let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("fig_cli"));
            cmd.arg("init").arg($shell).arg($stage).arg("--rcfile").arg($file);
            cmd.env("TMPDIR", "/fig-init-snapshot");
            let out = cmd.assert().success().get_output().stdout.clone();
            let init = String::from_utf8(out)?;
            insta::assert_snapshot!(init);
            Ok(())
        }
    };
}

init_test!(init_snapshot_zsh_pre_zshrc, "zsh", "pre", "zshrc");
init_test!(init_snapshot_zsh_pre_zprofile, "zsh", "pre", "zprofile");
init_test!(init_snapshot_zsh_post_zshrc, "zsh", "post", "zshrc");
init_test!(init_snapshot_zsh_post_zprofile, "zsh", "post", "zprofile");

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
