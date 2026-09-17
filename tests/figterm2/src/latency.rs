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
