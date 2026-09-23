//! Percentiles of per-report samples, and their spread across rounds.

use serde_json::{Map, Value, json};

const FIELDS: [&str; 6] = ["p50", "p95", "p99", "p999", "max", "mean"];

/// Nearest-rank percentiles, in nanoseconds.
#[derive(Clone, Copy)]
pub struct Distribution([f64; 6]);

impl Distribution {
    /// Sorts `samples`, which are in counter ticks.
    pub fn of(samples: &mut [u64], ns_per_tick: f64) -> Self {
        assert!(!samples.is_empty(), "no samples");
        samples.sort_unstable();
        let rank = |fraction: f64| {
            let index = (samples.len() as f64 * fraction).ceil() as usize;
            samples[index.clamp(1, samples.len()) - 1] as f64 * ns_per_tick
        };
        let mean = samples.iter().map(|&s| s as f64).sum::<f64>() / samples.len() as f64;
        Self([
            rank(0.5),
            rank(0.95),
            rank(0.99),
            rank(0.999),
            samples[samples.len() - 1] as f64 * ns_per_tick,
            mean * ns_per_tick,
        ])
    }

    pub fn json(&self) -> Value {
        let mut map = Map::new();
        for (field, value) in FIELDS.iter().zip(self.0) {
            map.insert((*field).into(), round(value));
        }
        Value::Object(map)
    }
}

pub fn round(value: f64) -> Value {
    json!((value * 10.0).round() / 10.0)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// For each percentile: the median over rounds, the lowest and highest round,
/// and the range as a percentage of the median.
pub fn across(rounds: &[Distribution]) -> Value {
    let mut map = Map::new();
    for (index, field) in FIELDS.iter().enumerate() {
        let mut values: Vec<f64> = rounds.iter().map(|round| round.0[index]).collect();
        let middle = median(&mut values);
        let (low, high) = (values[0], values[values.len() - 1]);
        map.insert(
            (*field).into(),
            json!({
                "median": round(middle),
                "min": round(low),
                "max": round(high),
                "spread_pct": round(if middle > 0.0 { (high - low) / middle * 100.0 } else { 0.0 }),
            }),
        );
    }
    Value::Object(map)
}

pub fn median_of(values: &[f64]) -> f64 {
    median(&mut values.to_vec())
}
