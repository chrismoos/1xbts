//! Extrapolate hardware time locally. `now_ns` runs on the bts-tx hot path and
//! must never take a lock.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering, fence};
use std::time::Instant;

use parking_lot::Mutex;

// Ignore small clock corrections to avoid following network jitter.
const SLEW_DEADBAND_NS: i64 = 1_000_000;
const SLEW_STEP_NS: i64 = 100_000;
const SNAP_THRESHOLD_NS: i64 = 50_000_000;

pub struct NetworkClock {
    epoch: Instant,
    offset_ns: AtomicI64,
    floor_ns: AtomicU64,
    /// Odd while a re-anchor is in progress.
    anchor_seq: AtomicU64,
    writer: Mutex<()>,
}

impl NetworkClock {
    pub fn new(server_ns: u64) -> Self {
        let epoch = Instant::now();
        Self {
            epoch,
            offset_ns: AtomicI64::new(offset_for(server_ns, 0)),
            floor_ns: AtomicU64::new(server_ns),
            anchor_seq: AtomicU64::new(0),
            writer: Mutex::new(()),
        }
    }

    fn local_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    fn estimate_ns(&self, offset_ns: i64) -> u64 {
        self.local_ns().saturating_add_signed(offset_ns)
    }

    pub fn now_ns(&self) -> u64 {
        loop {
            let seq = self.anchor_seq.load(Ordering::Acquire);
            if seq & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let offset_ns = self.offset_ns.load(Ordering::Relaxed);
            let floor_ns = self.floor_ns.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.anchor_seq.load(Ordering::Relaxed) != seq {
                continue;
            }
            let estimate = self.estimate_ns(offset_ns);
            // Never run backwards.
            if estimate <= floor_ns {
                return floor_ns;
            }
            if self
                .floor_ns
                .compare_exchange(floor_ns, estimate, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return estimate;
            }
        }
    }

    /// Server timestamps precede receipt by network transit time.
    pub fn observe_server_time(&self, server_ns: u64) {
        let _writer = self.writer.lock();
        let estimate = self.estimate_ns(self.offset_ns.load(Ordering::Relaxed));
        let diff = server_ns as i64 - estimate as i64;
        if diff.abs() < SLEW_DEADBAND_NS {
            return;
        }
        if diff.abs() > SNAP_THRESHOLD_NS {
            self.store_anchor(server_ns);
            return;
        }
        let step = diff.clamp(-SLEW_STEP_NS, SLEW_STEP_NS);
        self.offset_ns.fetch_add(step, Ordering::Relaxed);
    }

    pub fn reanchor(&self, server_ns: u64) {
        let _writer = self.writer.lock();
        self.store_anchor(server_ns);
    }

    fn store_anchor(&self, server_ns: u64) {
        let seq = self.anchor_seq.load(Ordering::Relaxed);
        self.anchor_seq
            .store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.offset_ns
            .store(offset_for(server_ns, self.local_ns()), Ordering::Relaxed);
        self.floor_ns.store(server_ns, Ordering::Relaxed);
        self.anchor_seq
            .store(seq.wrapping_add(2), Ordering::Release);
    }
}

fn offset_for(server_ns: u64, local_ns: u64) -> i64 {
    (server_ns as i64).wrapping_sub(local_ns as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn slew_ns(clock: &NetworkClock, server_ns: u64) -> i64 {
        clock.offset_ns.load(Ordering::Relaxed) - offset_for(server_ns, 0)
    }

    #[test]
    fn clock_advances_from_anchor() {
        let clock = NetworkClock::new(1_000_000_000);
        let t0 = clock.now_ns();
        assert!(t0 >= 1_000_000_000);
        std::thread::sleep(Duration::from_millis(2));
        let t1 = clock.now_ns();
        assert!(t1 > t0);
        assert!(t1 - t0 >= 2_000_000);
    }

    #[test]
    fn small_heartbeat_differences_are_ignored() {
        let clock = NetworkClock::new(1_000_000_000);
        let now = clock.now_ns();
        clock.observe_server_time(now + SLEW_DEADBAND_NS as u64 / 2);
        assert_eq!(slew_ns(&clock, 1_000_000_000), 0);
    }

    #[test]
    fn moderate_offsets_slew_in_bounded_steps() {
        let clock = NetworkClock::new(1_000_000_000);
        let now = clock.now_ns();
        clock.observe_server_time(now + 10_000_000);
        assert_eq!(slew_ns(&clock, 1_000_000_000), SLEW_STEP_NS);
        clock.observe_server_time(clock.now_ns().saturating_sub(10_000_000));
        assert_eq!(slew_ns(&clock, 1_000_000_000), 0);
    }

    #[test]
    fn large_offsets_snap_the_anchor() {
        let clock = NetworkClock::new(1_000_000_000);
        let far = clock.now_ns() + 10_000_000_000;
        clock.observe_server_time(far);
        assert!(clock.now_ns() >= far);
    }

    #[test]
    fn clock_is_monotonic_across_negative_slew() {
        let clock = NetworkClock::new(1_000_000_000);
        let t0 = clock.now_ns();
        clock.offset_ns.fetch_add(-1_000_000, Ordering::Relaxed);
        assert!(clock.now_ns() >= t0);
    }

    #[test]
    fn backward_reanchor_keeps_clock_running_from_new_time() {
        let clock = NetworkClock::new(10_000_000_000);
        assert!(clock.now_ns() >= 10_000_000_000);

        let earlier = 2_000_000_000;
        clock.reanchor(earlier);
        let t0 = clock.now_ns();
        assert!(t0 >= earlier && t0 < earlier + 1_000_000_000, "{t0}");
        std::thread::sleep(Duration::from_millis(2));
        let t1 = clock.now_ns();
        assert!(
            t1 - t0 >= 2_000_000,
            "clock froze after re-anchor: {t0} {t1}"
        );
    }

    #[test]
    fn backward_heartbeat_snap_keeps_clock_running() {
        let clock = NetworkClock::new(10_000_000_000);
        let earlier = clock.now_ns() - 2 * SNAP_THRESHOLD_NS as u64;
        clock.observe_server_time(earlier);
        let t0 = clock.now_ns();
        assert!(t0 < earlier + SNAP_THRESHOLD_NS as u64, "{t0}");
        std::thread::sleep(Duration::from_millis(2));
        assert!(clock.now_ns() > t0);
    }

    #[test]
    fn concurrent_readers_stay_monotonic_across_reanchors() {
        let clock = std::sync::Arc::new(NetworkClock::new(5_000_000_000));
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let clock = clock.clone();
                std::thread::spawn(move || {
                    for _ in 0..50_000 {
                        clock.now_ns();
                    }
                })
            })
            .collect();
        for step in 0..100u64 {
            clock.reanchor(1_000_000_000 + step * 1_000);
        }
        for reader in readers {
            reader.join().expect("reader thread");
        }
        clock.reanchor(1_000_000_000);
        let t0 = clock.now_ns();
        assert!(t0 < 1_500_000_000, "floor kept a pre-anchor value: {t0}");
    }
}
