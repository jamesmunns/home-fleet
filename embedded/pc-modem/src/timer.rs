use core::sync::atomic::{AtomicU32, Ordering};
use fleet_esb::RollingTimer;
use rtfm::{Fraction, Monotonic};

// pub const TICKS_PER_SECOND: u32 = 32768;
// pub const SIGNED_TICKS_PER_SECOND: i32 = 32768;

static RTC_STORE: AtomicU32 = AtomicU32::new(0);

pub struct RollingRtcTimer {
    time: &'static AtomicU32,
}

impl RollingRtcTimer {
    pub fn new() -> Self {
        Self { time: &RTC_STORE }
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

impl Monotonic for RollingRtcTimer {
    type Instant = i32;

    fn ratio() -> Fraction {
        // Note: This is the ratio between the LFCLK (32.768kHz)
        //   and the HFCLK (64 MHz)
        Fraction {
            numerator: 15_625,
            denominator: 8,
        }
    }
    fn now() -> Self::Instant {
        RTC_STORE.load(Ordering::Acquire) as i32
    }
    unsafe fn reset() {
        RTC_STORE.store(0, Ordering::Release);
    }

    fn zero() -> Self::Instant {
        0
    }
}
