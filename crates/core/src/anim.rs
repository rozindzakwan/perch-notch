//! Motion primitives: a damped spring for opening things, ease-out for closing
//! them. Both are stepped by explicit elapsed time so they are testable and
//! the event loop can sleep as soon as they settle.

use std::time::Duration;

/// Damping ratio: about 2.8 % overshoot on a 0 → 1 step (the spec allows 4 %).
const ZETA: f64 = 0.75;
/// `ln(1000)`: the spring counts as settled within 0.1 % of its target.
const SETTLE_LN: f64 = 6.907_755;

/// A critically under-damped spring driving one value toward a target.
///
/// Integrated in f64: an event flood can step it every few microseconds, and
/// at f32 precision `velocity * dt` then vanishes against a value of a few
/// hundred, so the value would freeze short of its target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    value: f64,
    velocity: f64,
    target: f64,
}

impl Spring {
    /// A spring at rest at `value`.
    pub fn new(value: f32) -> Self {
        Self {
            value: f64::from(value),
            velocity: 0.0,
            target: f64::from(value),
        }
    }

    /// Aim at a new target (keeps the current value and velocity, so
    /// retargeting mid-flight is smooth).
    pub fn set_target(&mut self, target: f32) {
        self.target = f64::from(target);
    }

    /// Jump straight to `value` and stop.
    pub fn snap(&mut self, value: f32) {
        self.value = f64::from(value);
        self.target = f64::from(value);
        self.velocity = 0.0;
    }

    /// Current value.
    pub fn value(&self) -> f32 {
        self.value as f32
    }

    /// Target value.
    pub fn target(&self) -> f32 {
        self.target as f32
    }

    /// At rest on the target.
    pub fn settled(&self) -> bool {
        (self.value - self.target).abs() < 0.001 && self.velocity.abs() < 0.01
    }

    /// Advance by `dt`, tuned so a full step settles in about `settle`.
    /// Returns true while still moving. Once settled the value is exactly the
    /// target.
    pub fn step(&mut self, dt: Duration, settle: Duration) -> bool {
        if self.settled() {
            self.rest();
            return false;
        }
        let secs = settle.as_secs_f64();
        if secs <= 0.0 {
            self.rest();
            return false;
        }
        let omega = SETTLE_LN / (ZETA * secs);
        let k = omega * omega;
        let c = 2.0 * ZETA * omega;
        // Semi-implicit Euler in sub-steps of at most 2 ms for accuracy.
        let mut left = dt.as_secs_f64();
        while left > 0.0 {
            let h = left.min(0.002);
            let accel = -k * (self.value - self.target) - c * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if self.settled() {
            self.rest();
            return false;
        }
        true
    }

    fn rest(&mut self) {
        self.value = self.target;
        self.velocity = 0.0;
    }
}

/// Cubic ease-out: fast start, gentle stop.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let f = 1.0 - t;
    1.0 - f * f * f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsecond_steps_still_settle() {
        // An event flood (X11 resize notifications) can step the spring every
        // few microseconds; at f32 precision a value near 300 then stops
        // moving while its velocity grows, and it never settles.
        let mut s = Spring::new(200.0);
        s.set_target(300.0);
        let settle = Duration::from_millis(320);
        let mut steps = 0u32;
        while s.step(Duration::from_micros(20), settle) {
            steps += 1;
            assert!(steps < 200_000, "not settled after 4 s of 20 µs steps");
        }
        assert_eq!(s.value(), 300.0);
        assert!(
            steps < 30_000,
            "settles in about 0.5 s of simulated time, took {} ms",
            steps / 50
        );
    }

    #[test]
    fn a_step_overshoots_slightly_and_settles_on_time() {
        let mut s = Spring::new(0.0);
        s.set_target(1.0);
        let mut max = 0.0_f32;
        let mut elapsed = 0;
        while s.step(Duration::from_millis(8), Duration::from_millis(320)) {
            max = max.max(s.value());
            elapsed += 8;
            assert!(elapsed < 1000, "never settled");
        }
        assert!(max > 1.0 && max <= 1.04, "overshoot {max}");
        assert!(elapsed <= 450, "settled after {elapsed} ms");
        assert_eq!(s.value(), 1.0);
    }

    #[test]
    fn retargeting_mid_flight_keeps_the_value() {
        let mut s = Spring::new(0.0);
        s.set_target(1.0);
        s.step(Duration::from_millis(80), Duration::from_millis(320));
        let mid = s.value();
        s.set_target(0.0);
        assert_eq!(s.value(), mid, "no jump on retarget");
    }

    #[test]
    fn ease_out_ends_where_it_should() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.5, "fast start");
    }
}
