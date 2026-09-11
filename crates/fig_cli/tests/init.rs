use std::process::Command;

use assert_cmd::prelude::*;
use paste::paste;

macro_rules! init_test {
    ($shell:expr, $stage:expr, $file:expr) => {
        paste! {
            fn [<init_output_ $shell _ $stage _ $file>]() -> Result<String, Box<dyn std::error::Error>> {
                let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("fig_cli"));
                cmd.arg("init").arg($shell).arg($stage).arg("--rcfile").arg($file);
                cmd.env("FIG_INIT_SNAPSHOT_TEST", "1");
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

// bash
init_test!("bash", "pre", "bashrc");
init_test!("bash", "pre", "bash_profile");
init_test!("bash", "post", "bashrc");
init_test!("bash", "post", "bash_profile");

// zsh
init_test!("zsh", "pre", "zshrc");
init_test!("zsh", "pre", "zprofile");
init_test!("zsh", "post", "zshrc");
init_test!("zsh", "post", "zprofile");
