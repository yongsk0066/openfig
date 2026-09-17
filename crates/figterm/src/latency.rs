//! Trace-level timestamps along the keystroke path (docs/16-latency.md, `just latency-segments`).
//!
//! Enabled with `FIG_LOG_LEVEL=figterm::latency=trace`. Every mark is a `trace!` on [`TARGET`]
//! whose message names the point and whose `t_us` field is wall-clock microseconds: the two ends
//! of the measurement live in other processes (the harness writing the key, the stub desktop
//! decoding the hook), and the wall clock is the one clock all three read without `unsafe`.
//! `tracing` evaluates field values only for an enabled callsite, so while the target is off a
//! mark is a callsite-interest check and `now_us` never runs.
//!
//! Points, in keystroke order: `stdin_read` → `input_parsed` → `input_recv` → `pty_write` →
//! `pty_read` → `parsed` → `echoed` → `edit_buffer` → `hook_queued`.

pub(crate) const TARGET: &str = "figterm::latency";

pub(crate) fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_micros()).unwrap_or(u64::MAX))
}
