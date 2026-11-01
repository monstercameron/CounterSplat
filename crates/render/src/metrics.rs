//! Bounded CPU metrics shared by native, browser and simulation. All durations are milliseconds.
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
#[derive(Default)]
pub struct Metrics {
    samples: BTreeMap<String, VecDeque<f64>>,
    counters: BTreeMap<String, u64>,
}
impl Metrics {
    pub fn record(&mut self, name: &str, ms: f64) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        // Borrow the lookup key on the hot path. BTreeMap::entry would allocate
        // a new String on every measurement, even when this series exists.
        let s = if let Some(samples) = self.samples.get_mut(name) {
            samples
        } else {
            self.samples.entry(name.into()).or_default()
        };
        if s.len() == 120 {
            s.pop_front();
        }
        s.push_back(ms);
    }
    pub fn count(&mut self, name: &str, n: u64) {
        if let Some(counter) = self.counters.get_mut(name) {
            *counter += n;
        } else {
            self.counters.insert(name.into(), n);
        }
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn json(&self) -> Value {
        json!({"window_capacity":120,"cpu_ms":self.samples.iter().map(|(k,v)|(k.clone(),summary(v.iter().copied()))).collect::<BTreeMap<_,_>>(),"counters":self.counters})
    }
}
pub(crate) fn summary(values: impl Iterator<Item = f64>) -> Value {
    let mut v: Vec<_> = values.collect();
    if v.is_empty() {
        return Value::Null;
    }
    let latest = *v.last().unwrap();
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    v.sort_by(f64::total_cmp);
    json!({"samples":v.len(),"latest":latest,"mean":mean,"min":v[0],"max":v[v.len()-1],"p50":v[v.len()/2],"p95":v[((v.len()-1)*95).div_ceil(100)]})
}
/// Monotonic native clock. Browser hosts supply performance.now instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn clock_ms() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_window_and_percentiles() {
        let mut m = Metrics::default();
        for i in 0..200 {
            m.record("encode", i as f64);
        }
        m.record("encode", f64::NAN);
        m.count("frames", 200);
        let j = m.json();
        assert_eq!(j["cpu_ms"]["encode"]["samples"], 120);
        assert_eq!(j["cpu_ms"]["encode"]["min"], 80.0);
        assert_eq!(j["cpu_ms"]["encode"]["p95"], 194.0);
        assert_eq!(j["counters"]["frames"], 200);
        m.reset();
        assert!(m.json()["cpu_ms"].as_object().unwrap().is_empty());
    }
}
