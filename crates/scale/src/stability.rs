//! Motion detection. A weight is only "stable" when the indicator says so (if it
//! reports a flag at all) AND the readings have stayed within `tolerance_kg`
//! for `window_ms`. This stops a ticket being captured while a truck is still
//! rolling on or off the deck.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StabilityConfig {
    #[serde(default = "d_window")]
    pub window_ms: u64,
    #[serde(default = "d_tol")]
    pub tolerance_kg: f64,
    #[serde(default = "d_min")]
    pub min_samples: usize,
}

fn d_window() -> u64 {
    1500
}
fn d_tol() -> f64 {
    10.0
}
fn d_min() -> usize {
    3
}

impl Default for StabilityConfig {
    fn default() -> Self {
        Self { window_ms: d_window(), tolerance_kg: d_tol(), min_samples: d_min() }
    }
}

pub struct Stability {
    cfg: StabilityConfig,
    samples: VecDeque<(Instant, f64)>,
}

impl Stability {
    pub fn new(cfg: StabilityConfig) -> Self {
        Self { cfg, samples: VecDeque::new() }
    }

    pub fn push(&mut self, at: Instant, weight: f64, indicator: Option<bool>) -> bool {
        let window = Duration::from_millis(self.cfg.window_ms);
        self.samples.push_back((at, weight));
        // keep slightly more than the window so we can tell the span was covered
        while let Some((t, _)) = self.samples.front() {
            if at.duration_since(*t) > window + Duration::from_millis(250) {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        if indicator == Some(false) {
            return false;
        }
        let in_window: Vec<f64> = self
            .samples
            .iter()
            .filter(|(t, _)| at.duration_since(*t) <= window)
            .map(|(_, w)| *w)
            .collect();
        let covered = self
            .samples
            .front()
            .map(|(t, _)| at.duration_since(*t) >= window.mul_f32(0.8))
            .unwrap_or(false);
        if in_window.len() < self.cfg.min_samples || !covered {
            return false;
        }
        let (lo, hi) = in_window.iter().fold((f64::MAX, f64::MIN), |(lo, hi), w| (lo.min(*w), hi.max(*w)));
        hi - lo <= self.cfg.tolerance_kg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn becomes_stable_after_window() {
        let mut s = Stability::new(StabilityConfig::default());
        let t0 = Instant::now();
        let mut last = false;
        for i in 0..20 {
            last = s.push(t0 + Duration::from_millis(i * 100), 12340.0, None);
            if i < 12 {
                assert!(!last, "stable too early at {i}");
            }
        }
        assert!(last);
    }

    #[test]
    fn moving_is_unstable_and_indicator_overrides() {
        let mut s = Stability::new(StabilityConfig::default());
        let t0 = Instant::now();
        for i in 0..20 {
            assert!(!s.push(t0 + Duration::from_millis(i * 100), 10000.0 + i as f64 * 100.0, None));
        }
        let mut s = Stability::new(StabilityConfig::default());
        for i in 0..20 {
            assert!(!s.push(t0 + Duration::from_millis(i * 100), 500.0, Some(false)));
        }
    }
}
