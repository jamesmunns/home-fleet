use core::sync::atomic::{AtomicU32, Ordering};
use fleet_esb::RollingTimer;

pub const TICKS_PER_SECOND: u32 = 32768;

pub struct RollingRtcTimer {
    time: &'static AtomicU32,
}

impl RollingRtcTimer {
    pub fn new(store: &'static AtomicU32) -> Self {
        Self {
            time: store
        }
    }

    // Should be called by the rtc interrupt
    pub fn tick(&self) {
        self.time.fetch_add(1, Ordering::Release);
    }
}

impl RollingTimer for RollingRtcTimer {
    fn get_current_tick(&self) -> u32 {
        self.time.load(Ordering::Acquire)
    }
}
