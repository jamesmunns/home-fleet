use crate::hal::gpio::{OpenDrain, Output, Pin};
use crate::timer::TICKS_PER_SECOND;
use embedded_hal::digital::v2::OutputPin;
use embedded_hal::digital::v2::StatefulOutputPin;
use fleet_esb::RollingTimer;
use fleet_icd::radio::{RelayIdx, RelayState};

const MIN_TOGGLE_DELTA: u32 = 3 * TICKS_PER_SECOND;
const COMMS_TIMEOUT: u32 = 60 * TICKS_PER_SECOND;

pub struct Relays<T>
where
    T: RollingTimer,
{
    relays: [Relay; 4],
    timer: T,
    last_message_ms: u32,
}

pub struct Relay {
    gpio: Pin<Output<OpenDrain>>,
    last_toggle_ms: u32,
}

impl Relay {
    fn from_pin(pin: Pin<Output<OpenDrain>>, now: u32) -> Self {
        Self {
            gpio: pin,
            last_toggle_ms: now,
        }
    }
}

impl<T> Relays<T>
where
    T: RollingTimer,
{
    pub fn from_pins(pins: [Pin<Output<OpenDrain>>; 4], timer: T) -> Self {
        let now = timer.get_current_tick();
        let [mut pin_0, mut pin_1, mut pin_2, mut pin_3] = pins;

        // Make sure all pins are off at startup
        pin_0.set_high().ok();
        pin_1.set_high().ok();
        pin_2.set_high().ok();
        pin_3.set_high().ok();

        Self {
            relays: [
                Relay::from_pin(pin_0, now),
                Relay::from_pin(pin_1, now),
                Relay::from_pin(pin_2, now),
                Relay::from_pin(pin_3, now),
            ],
            timer,
            last_message_ms: now,
        }
    }

    pub fn set_relay(&mut self, relay: RelayIdx, state: RelayState) -> Result<(), ()> {
        let relay = match relay {
            RelayIdx::Relay0 => &mut self.relays[0],
            RelayIdx::Relay1 => &mut self.relays[1],
            RelayIdx::Relay2 => &mut self.relays[2],
            RelayIdx::Relay3 => &mut self.relays[3],
        };

        let now = self.timer.get_current_tick();
        let delta = now.wrapping_sub(relay.last_toggle_ms);

        if delta <= MIN_TOGGLE_DELTA {
            return Err(());
        }

        let is_low = relay.gpio.is_set_low().map_err(drop)?;

        match state {
            RelayState::On if !is_low => {
                relay.gpio.set_low().ok();
                relay.last_toggle_ms = now;
            }
            RelayState::Off if is_low => {
                relay.gpio.set_high().ok();
                relay.last_toggle_ms = now;
            }
            _ => {}
        }

        self.last_message_ms = now;

        Ok(())
    }

    pub fn check_timeout(&mut self) {
        let now = self.timer.get_current_tick();
        let delta = now.wrapping_sub(self.last_message_ms);

        if delta >= COMMS_TIMEOUT {
            self.relays.iter_mut().for_each(|r| {
                r.gpio.set_low().ok();
                r.last_toggle_ms = now;
            });
        }
    }

    pub fn current_state(&self) -> [RelayState; 4] {
        [
            if self.relays[0].gpio.is_set_low().unwrap() {
                RelayState::On
            } else {
                RelayState::Off
            },
            if self.relays[1].gpio.is_set_low().unwrap() {
                RelayState::On
            } else {
                RelayState::Off
            },
            if self.relays[2].gpio.is_set_low().unwrap() {
                RelayState::On
            } else {
                RelayState::Off
            },
            if self.relays[3].gpio.is_set_low().unwrap() {
                RelayState::On
            } else {
                RelayState::Off
            },
        ]
    }
}
