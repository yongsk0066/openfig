//! Order statistics for the latency benchmarks.

use std::fmt;
use std::time::Duration;

/// Nearest-rank summary of a set of samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub n: usize,
    pub min: Duration,
    pub median: Duration,
    pub p90: Duration,
    pub p99: Duration,
    pub max: Duration,
}

impl Summary {
    /// `None` for an empty set.
    pub fn of(samples: &[Duration]) -> Option<Summary> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        Some(Summary {
            n: sorted.len(),
            min: sorted[0],
            median: percentile(&sorted, 50),
            p90: percentile(&sorted, 90),
            p99: percentile(&sorted, 99),
            max: sorted[sorted.len() - 1],
        })
    }
}

/// Nearest rank: the smallest sample with at least `p` % of the set at or below it. `sorted` must
/// be sorted and non-empty.
fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        write!(
            f,
            "n={} min={:.2}ms median={:.2}ms p90={:.2}ms p99={:.2}ms max={:.2}ms",
            self.n,
            ms(self.min),
            ms(self.median),
            ms(self.p90),
            ms(self.p99),
            ms(self.max)
        )
    }
}

/// The points the pty binary marks on `figterm::latency` (`crates/figterm/src/latency.rs`), in the
/// order a keystroke passes them.
pub const POINTS: &[&str] = &[
    "stdin_read",
    "input_parsed",
    "input_recv",
    "pty_write",
    "pty_read",
    "parsed",
    "echoed",
    "edit_buffer",
    "hook_queued",
];

/// `(point, t_us)` for every `figterm::latency` line of a pty binary log, in file order. The log
/// file is written with ANSI colors (`fig_log` does not turn them off for files), so SGR sequences
/// are dropped first.
pub fn parse_marks(log: &str) -> Vec<(&'static str, u64)> {
    log.lines()
        .map(strip_sgr)
        .filter(|line| line.contains("figterm::latency"))
        .filter_map(|line| {
            let point = line
                .split_whitespace()
                .find_map(|word| POINTS.iter().find(|point| **point == word))?;
            let t_us = line.split_once("t_us=")?.1;
            let digits = t_us.find(|c: char| !c.is_ascii_digit()).unwrap_or(t_us.len());
            Some((*point, t_us[..digits].parse().ok()?))
        })
        .collect()
}

/// `line` without `ESC [ … m`.
fn strip_sgr(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("\x1b[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('m') {
            Some(end) if after[..end].bytes().all(|b| b.is_ascii_digit() || b == b';') => rest = &after[end + 1..],
            _ => {
                out.push_str("\x1b[");
                rest = after;
            },
        }
    }
    out.push_str(rest);
    out
}

/// For a key written at `written_us`, the time of each of [`POINTS`]: the first mark of that point
/// at or after the previous point, all before `until_us` (the next key). `None` if any is missing.
pub fn chain(marks: &[(&str, u64)], written_us: u64, until_us: u64) -> Option<Vec<u64>> {
    let mut at = written_us;
    POINTS
        .iter()
        .map(|point| {
            let (_, t) = marks
                .iter()
                .find(|(name, t)| name == point && *t >= at && *t < until_us)?;
            at = *t;
            Some(*t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_millis).collect()
    }

    #[test]
    fn nearest_rank() {
        let s = Summary::of(&ms(&[10, 1, 9, 2, 8, 3, 7, 4, 6, 5])).unwrap();
        assert_eq!(s.n, 10);
        assert_eq!(s.min, Duration::from_millis(1));
        assert_eq!(s.median, Duration::from_millis(5));
        assert_eq!(s.p90, Duration::from_millis(9));
        assert_eq!(s.p99, Duration::from_millis(10));
        assert_eq!(s.max, Duration::from_millis(10));
    }

    #[test]
    fn marks_chain_in_point_order() {
        let log = "\
2026-09-17T12:00:00.000001Z TRACE figterm::latency: 360: stdin_read t_us=100 n=1
2026-09-17T12:00:00.000002Z TRACE figterm::latency: 340: input_parsed t_us=1300
2026-09-17T12:00:00.000003Z DEBUG figterm: 900: pty_read t_us=1 not a latency line
2026-09-17T12:00:00.000004Z TRACE figterm::latency: 830: input_recv t_us=1310
2026-09-17T12:00:00.000005Z TRACE figterm::latency: 925: pty_write t_us=1320 n=1
2026-09-17T12:00:00.000006Z TRACE figterm::latency: 941: pty_read t_us=1400 n=1
2026-09-17T12:00:00.000007Z TRACE figterm::latency: 944: parsed t_us=1410
2026-09-17T12:00:00.000008Z TRACE figterm::latency: 948: echoed t_us=1450
2026-09-17T12:00:00.000009Z TRACE figterm::latency: 951: edit_buffer t_us=1460 some=true
2026-09-17T12:00:00.000010Z TRACE figterm::latency: 466: hook_queued t_us=1470
2026-09-17T12:00:00.000011Z TRACE figterm::latency: 941: pty_read t_us=9000 n=1
";
        let marks = parse_marks(log);
        assert_eq!(marks.len(), 10);
        assert_eq!(
            parse_marks(
                "\x1b[2m2026-09-17T13:42:43.366289Z\x1b[0m \x1b[35mTRACE\x1b[0m \x1b[2mfigterm::latency\x1b[0m\x1b[2m:\x1b[0m \x1b[2m944:\x1b[0m pty_read \x1b[3mt_us\x1b[0m\x1b[2m=\x1b[0m1789652563366285 \x1b[3mn\x1b[0m\x1b[2m=\x1b[0m293"
            ),
            vec![("pty_read", 1789652563366285)]
        );
        assert_eq!(
            chain(&marks, 50, 5000),
            Some(vec![100, 1300, 1310, 1320, 1400, 1410, 1450, 1460, 1470])
        );
        // The next key is written before the hook is queued: this key's chain is incomplete.
        assert_eq!(chain(&marks, 50, 1465), None);
        assert_eq!(chain(&marks, 200, 5000), None);
    }

    #[test]
    fn single_sample_and_empty() {
        let s = Summary::of(&ms(&[7])).unwrap();
        assert_eq!(
            (s.median, s.p90, s.p99),
            (
                Duration::from_millis(7),
                Duration::from_millis(7),
                Duration::from_millis(7)
            )
        );
        assert_eq!(Summary::of(&[]), None);
    }
}
