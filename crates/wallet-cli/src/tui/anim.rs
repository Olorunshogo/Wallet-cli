//! Animation primitives. Every function takes the current time as an
//! argument instead of reading the clock, so tests can step time precisely.

use std::time::{Duration, Instant};

/// Braille spinner frames.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Time each spinner frame is shown.
pub const SPINNER_FRAME: Duration = Duration::from_millis(80);

/// The spinner frame for a task that started at `start`.
pub fn spinner(start: Instant, now: Instant) -> &'static str {
    let step = now.saturating_duration_since(start).as_millis() / SPINNER_FRAME.as_millis();
    SPINNER[step as usize % SPINNER.len()]
}

/// Ease-out cubic: fast start, gentle landing.
pub fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A number animating from one value to another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    from: u64,
    to: u64,
    start: Instant,
    duration: Duration,
}

impl Tween {
    /// A tween already at rest on `value`.
    pub fn at(value: u64, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            duration: Duration::ZERO,
        }
    }

    /// Start moving toward `target` from wherever the value is now.
    pub fn retarget(&mut self, target: u64, now: Instant, duration: Duration) {
        if target == self.to {
            return;
        }
        self.from = self.value(now);
        self.to = target;
        self.start = now;
        self.duration = duration;
    }

    /// The value to display at `now`.
    pub fn value(&self, now: Instant) -> u64 {
        if self.duration.is_zero() {
            return self.to;
        }
        let t =
            now.saturating_duration_since(self.start).as_secs_f64() / self.duration.as_secs_f64();
        let eased = ease_out(t);
        let (from, to) = (self.from as f64, self.to as f64);
        (from + (to - from) * eased).round() as u64
    }
}

/// Which cells of a skeleton bar are lit by the moving shine at `now`.
///
/// A band a quarter of the width sweeps left to right every 1.2 s.
pub fn shimmer(width: u16, start: Instant, now: Instant) -> Vec<bool> {
    const PERIOD_MS: u128 = 1_200;
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    let band = (width / 4).max(1);
    let phase = now.saturating_duration_since(start).as_millis() % PERIOD_MS;
    let span = width + band;
    let head = (phase * span as u128 / PERIOD_MS) as usize;
    (0..width).map(|i| i + band > head && i <= head).collect()
}

/// How far a notification has slid in (0 = hidden, 1 = fully shown), and
/// whether it has expired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lifecycle {
    /// Slide position in `0.0..=1.0`.
    pub shown: f64,
    /// True once the notification should be removed.
    pub expired: bool,
}

/// Slide in for 200 ms, stay, slide out for the last 300 ms of `ttl`.
pub fn lifecycle(created: Instant, ttl: Duration, now: Instant) -> Lifecycle {
    const IN: f64 = 0.2;
    const OUT: f64 = 0.3;
    let age = now.saturating_duration_since(created).as_secs_f64();
    let ttl = ttl.as_secs_f64();
    if age >= ttl {
        return Lifecycle {
            shown: 0.0,
            expired: true,
        };
    }
    let shown = if age < IN {
        ease_out(age / IN)
    } else if age > ttl - OUT {
        ease_out((ttl - age) / OUT)
    } else {
        1.0
    };
    Lifecycle {
        shown,
        expired: false,
    }
}

/// True for a short time after `since`, used to flash new rows.
pub fn flashing(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) < Duration::from_millis(1_500)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn spinner_cycles_through_frames() {
        let t0 = Instant::now();
        assert_eq!(spinner(t0, t0), SPINNER[0]);
        assert_eq!(spinner(t0, t0 + ms(80)), SPINNER[1]);
        assert_eq!(
            spinner(t0, t0 + ms(800)),
            SPINNER[0],
            "wraps after 10 frames"
        );
    }

    #[test]
    fn tween_moves_then_settles() {
        let t0 = Instant::now();
        let mut tween = Tween::at(0, t0);
        tween.retarget(1_000, t0, ms(1_000));
        assert_eq!(tween.value(t0), 0);
        let mid = tween.value(t0 + ms(500));
        assert!(
            mid > 500 && mid < 1_000,
            "ease-out is past halfway at t=0.5: {mid}"
        );
        assert_eq!(tween.value(t0 + ms(1_000)), 1_000);
        assert_eq!(tween.value(t0 + ms(2_000)), 1_000, "stays settled");
    }

    #[test]
    fn retarget_continues_from_the_current_value() {
        let t0 = Instant::now();
        let mut tween = Tween::at(0, t0);
        tween.retarget(1_000, t0, ms(1_000));
        let shown = tween.value(t0 + ms(500));
        tween.retarget(0, t0 + ms(500), ms(1_000));
        assert_eq!(tween.value(t0 + ms(500)), shown, "no jump when retargeting");
        assert_eq!(tween.value(t0 + ms(1_500)), 0);
    }

    #[test]
    fn shimmer_band_moves_across() {
        let t0 = Instant::now();
        let early = shimmer(20, t0, t0 + ms(100));
        let late = shimmer(20, t0, t0 + ms(900));
        let first_lit = |v: &[bool]| v.iter().position(|&b| b);
        assert!(first_lit(&late) > first_lit(&early));
        assert!(early.iter().filter(|&&b| b).count() <= 5);
        assert!(shimmer(0, t0, t0).is_empty());
    }

    #[test]
    fn notifications_slide_in_hold_and_expire() {
        let t0 = Instant::now();
        let ttl = Duration::from_secs(4);
        assert!(lifecycle(t0, ttl, t0 + ms(50)).shown < 1.0);
        assert_eq!(lifecycle(t0, ttl, t0 + ms(2_000)).shown, 1.0);
        assert!(lifecycle(t0, ttl, t0 + ms(3_900)).shown < 1.0);
        assert!(lifecycle(t0, ttl, t0 + ms(4_000)).expired);
        assert!(flashing(t0, t0 + ms(1_000)));
        assert!(!flashing(t0, t0 + ms(2_000)));
    }
}
