//! Edit-buffer goldens (`zsh_hook_tests::golden`, docs/15-architecture-study.md §4 stage 2).
//!
//! `golden_record` regenerates `tests/zsh-hook/ref/fig_*/edit_buffer.json` and is what
//! `just golden-record` runs; the zsh hook replays every case in `hook.rs`.

use zsh_hook_tests::golden;

/// Records every case, or the comma-separated `FIG_GOLDEN_CASES`. Does nothing unless
/// `FIG_GOLDEN_RECORD=1`, so `--run-ignored all` cannot rewrite committed recordings by accident.
#[test]
#[ignore = "rewrites tests/zsh-hook/ref: `just golden-record`"]
fn golden_record() {
    if std::env::var("FIG_GOLDEN_RECORD").as_deref() != Ok("1") {
        println!("golden_record: FIG_GOLDEN_RECORD=1 is not set, nothing recorded");
        return;
    }
    let only = std::env::var("FIG_GOLDEN_CASES").unwrap_or_default();
    let only: Vec<&str> = only.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    for name in &only {
        assert!(golden::case(name).is_some(), "no golden case named {name:?}");
    }

    let cli = zsh_hook_tests::cli_binary().unwrap();
    for case in golden::CASES {
        if !only.is_empty() && !only.contains(&case.name) {
            continue;
        }
        let out = golden::ref_dir().join(case.name);
        golden::record(case, &cli, &out).unwrap_or_else(|err| panic!("{}: {err:?}", case.name));
        println!("golden_record: {} -> {}", case.name, out.display());
    }
}
