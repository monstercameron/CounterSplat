//! Validated rational rates for clocks and deadline pacing.
//!
//! A rational rate represents both 60 Hz and rates such as 60000/1001 Hz without accumulating
//! a rounded frame duration on every frame.

use std::time::Duration;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A positive rate in events per second, stored as a reduced numerator/denominator pair.
///
/// Rates faster than one event per nanosecond are rejected because `Duration` cannot distinguish
/// their deadlines. Both fields are private so every constructed value satisfies the invariants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    numerator: u32,
    denominator: u32,
}

impl Rate {
    /// Construct an exact rational rate in events per second.
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, &'static str> {
        if numerator == 0 || denominator == 0 {
            return Err("rate numerator and denominator must be nonzero");
        }
        if u128::from(numerator) > NANOS_PER_SECOND * u128::from(denominator) {
            return Err("rate must not exceed one billion events per second");
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    /// Approximate a finite positive frequency. Use [`Self::new`] for an exact fractional rate.
    pub fn from_hz(hz: f64) -> Result<Self, &'static str> {
        if !hz.is_finite() || hz <= 0.0 || hz > NANOS_PER_SECOND as f64 {
            return Err("rate must be finite, positive and no more than one billion hertz");
        }
        let mut denominator = if hz < 0.000_001 { u32::MAX } else { 1_000_000 };
        while hz * f64::from(denominator) > f64::from(u32::MAX) && denominator > 1 {
            denominator = (denominator / 10).max(1);
        }
        let numerator = (hz * f64::from(denominator)).round();
        if numerator < 1.0 || numerator > f64::from(u32::MAX) {
            return Err("rate cannot be represented");
        }
        Self::new(numerator as u32, denominator)
    }

    /// Numerator of the reduced rate, in events.
    pub const fn numerator(self) -> u32 {
        self.numerator
    }
    /// Denominator of the reduced rate, in seconds.
    pub const fn denominator(self) -> u32 {
        self.denominator
    }
    /// Rate as a floating-point frequency, for diagnostics.
    pub fn hz(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }
    /// Exact period as seconds, for passing a fixed step to a simulation.
    pub fn period_seconds(self) -> f64 {
        f64::from(self.denominator) / f64::from(self.numerator)
    }
    /// Period rounded to the closest nanosecond. Deadlines use the exact rational rate instead.
    pub fn period(self) -> Duration {
        let numerator = self.period_units();
        let rate = u128::from(self.numerator);
        duration_from_nanos((numerator + rate / 2) / rate)
            .expect("validated rate has representable period")
    }
    pub(crate) fn period_units(self) -> u128 {
        NANOS_PER_SECOND * u128::from(self.denominator)
    }
    pub(crate) fn scale(self) -> u128 {
        u128::from(self.numerator)
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

pub(crate) fn duration_from_nanos(nanos: u128) -> Option<Duration> {
    if nanos > Duration::MAX.as_nanos() {
        return None;
    }
    Some(Duration::new(
        (nanos / NANOS_PER_SECOND) as u64,
        (nanos % NANOS_PER_SECOND) as u32,
    ))
}

pub(crate) fn ceil_div(value: u128, divisor: u128) -> u128 {
    value.div_ceil(divisor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_and_reduces_rates() {
        assert_eq!(Rate::new(120, 2).unwrap(), Rate::new(60, 1).unwrap());
        assert_eq!(Rate::from_hz(60.0).unwrap(), Rate::new(60, 1).unwrap());
        assert!(Rate::new(0, 1).is_err());
        assert!(Rate::new(1, 0).is_err());
        assert!(Rate::new(1_000_000_001, 1).is_err());
        assert!(Rate::from_hz(f64::NAN).is_err());
        assert!(Rate::from_hz(f64::INFINITY).is_err());
        assert!(Rate::from_hz(0.0).is_err());
        assert!(Rate::from_hz(1e-12).is_err());
    }
}
