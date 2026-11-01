//! Deterministic stress tests using a reference integer timeline and repeatable jitter.
use countersplat_core::{
    pacing::{DeadlinePacer, FramePacer, PacingMode},
    rate::Rate,
    scheduler::Scheduler,
    time::FixedClock,
};
use std::time::Duration;

fn next_random(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    *seed
}

#[test]
fn long_jittered_timeline_matches_exact_integer_reference() {
    let rate = Rate::new(60_000, 1_001).unwrap();
    let mut clock = FixedClock::with_rate(rate, Duration::from_secs(1), 100).unwrap();
    let mut pacer = DeadlinePacer::new(rate);
    let mut seed = 0x1234_5678_9abc_def0;
    let mut now_ns = 0u128;
    let mut total_ticks = 0u128;
    let mut last_rendered_index = 0u128;
    assert!(pacer.poll(Duration::ZERO).unwrap().due);
    let period = 1_000_000_000u128 * u128::from(rate.denominator());
    let scale = u128::from(rate.numerator());
    for _ in 0..100_000 {
        let delta = next_random(&mut seed) % 80_000_001;
        now_ns += u128::from(delta);
        let step = clock.advance(Duration::from_nanos(delta));
        total_ticks += u128::from(step.ticks);
        let expected = now_ns * scale / period;
        assert_eq!(total_ticks, expected, "fixed clock drifted at {now_ns} ns");
        assert_eq!(step.dropped, Duration::ZERO);
        assert!((0.0..1.0).contains(&step.alpha));
        let decision = pacer.poll(Duration::from_nanos(now_ns as u64)).unwrap();
        if expected > last_rendered_index {
            assert!(decision.due);
            assert_eq!(decision.missed, expected - last_rendered_index - 1);
            last_rendered_index = expected;
        } else {
            assert!(!decision.due);
        }
        assert!(decision
            .next_deadline
            .is_none_or(|deadline| deadline.as_nanos() > now_ns));
    }
}

#[test]
fn rate_change_does_not_create_an_extra_frame() {
    let mut scheduler = Scheduler::new(
        FixedClock::new(60.0, Duration::from_secs(1), 10).unwrap(),
        Rate::new(60, 1).unwrap(),
    );
    assert!(scheduler.poll(Duration::ZERO).unwrap().frame.due);
    let change = Duration::from_nanos(16_666_667);
    assert!(scheduler.poll(change).unwrap().frame.due);
    assert!(scheduler
        .set_frame_rate(Rate::new(30, 1).unwrap(), Duration::ZERO)
        .is_err());
    scheduler
        .set_frame_rate(Rate::new(30, 1).unwrap(), change)
        .unwrap();
    assert!(!scheduler.poll(change).unwrap().frame.due);
    assert!(!scheduler.poll(Duration::from_millis(49)).unwrap().frame.due);
    assert!(
        scheduler
            .poll(Duration::from_nanos(50_000_001))
            .unwrap()
            .frame
            .due
    );
}

#[test]
fn callback_pacer_ignores_nonfinite_and_backwards_inputs() {
    assert_eq!(PacingMode::parse("inf"), PacingMode::Auto);
    let mut pacer = FramePacer::new(PacingMode::Fixed(f64::NAN));
    pacer.set_refresh_hint(Some(f64::INFINITY));
    pacer.record_gpu_ms(f64::NAN);
    pacer.record_gpu_ms(f64::INFINITY);
    assert_eq!(pacer.gpu_ms(), 0.0);
    assert!(pacer.tick(100.0));
    assert!(!pacer.tick(f64::NAN));
    assert!(!pacer.tick(99.0));
    assert!(pacer.tick(117.0));
}

#[test]
fn extreme_valid_rates_and_duration_limits_are_safe() {
    for rate in [
        Rate::new(1, u32::MAX).unwrap(),
        Rate::new(1_000_000_000, 1).unwrap(),
        Rate::new(u32::MAX, u32::MAX).unwrap(),
    ] {
        let mut clock = FixedClock::with_rate(rate, Duration::MAX, 2).unwrap();
        let result = clock.advance(Duration::MAX);
        assert!(result.ticks <= 2);
        assert!((0.0..1.0).contains(&result.alpha));
        let mut pacer = DeadlinePacer::new(rate);
        assert!(pacer.poll(Duration::ZERO).unwrap().due);
        let _ = pacer.poll(Duration::MAX).unwrap();
        assert!(!pacer.poll(Duration::MAX).unwrap().due);
    }
}
