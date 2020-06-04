#![no_std]
#![no_main]

// We need to import this crate explicitly so we have a panic handler
//use panic_semihosting as _;

/// Configuration macro to be called by the user configuration in `config.rs`.
///
/// Expands to yet another `apply_config!` macro that's called from `init` and performs some
/// hardware initialization based on the config values.
macro_rules! config {
    (
        baudrate = $baudrate:ident;
        tx_pin = $tx_pin:ident;
        rx_pin = $rx_pin:ident;
    ) => {
        macro_rules! apply_config {
            ( $p0:ident, $uart:ident ) => {{
                let rxd = $p0.$rx_pin.into_floating_input().degrade();
                let txd = $p0.$tx_pin.into_push_pull_output(Level::Low).degrade();

                let pins = hal::uarte::Pins {
                    rxd,
                    txd,
                    cts: None,
                    rts: None,
                };

                hal::uarte::Uarte::new($uart, pins, Parity::EXCLUDED, Baudrate::$baudrate)
            }};
        }
    };
}

#[macro_use]
mod config;

mod relays;
mod timer;

// Import the right HAL/PAC crate, depending on the target chip
#[cfg(feature = "51")]
use nrf51_hal as hal;
#[cfg(feature = "52810")]
use nrf52810_hal as hal;
#[cfg(feature = "52832")]
use nrf52832_hal as hal;
#[cfg(feature = "52840")]
use nrf52840_hal as hal;

use {
    core::{
        default::Default,
        fmt::Write,
        sync::atomic::{AtomicBool, AtomicU32, Ordering},
    },
    esb::{
        consts::*, irq::StatePTX, Addresses, BBBuffer, ConfigBuilder, ConstBBBuffer, Error,
        EsbBuffer, EsbIrq, IrqTimer,
    },
    hal::{
        clocks::LfOscConfiguration,
        gpio::{Level, OpenDrainConfig},
        pac::{RTC0, TIMER0, TIMER1, UARTE0},
        rtc::{RtcInterrupt, Started},
        target::NVIC,
        uarte::{Baudrate, Parity, Uarte},
        Rng, Rtc, Timer,
    },
    rtt_target::{rprintln, rtt_init_print},
};

use fleet_esb::{ptx::FleetRadioPtx, RxMessage};

use fleet_icd::radio::{DeviceToHost, GeneralDeviceMessage, HostToDevice};

use embedded_hal::blocking::delay::DelayMs;

use relays::Relays;
use timer::RollingRtcTimer;

static RTC_STORE: AtomicU32 = AtomicU32::new(0);

static ATTEMPTS_FLAG: AtomicBool = AtomicBool::new(false);

#[rtfm::app(device = crate::hal::pac, peripherals = true)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPtx<U8192, U8192, DeviceToHost, HostToDevice, RollingRtcTimer>,
        esb_irq: EsbIrq<U8192, U8192, TIMER0, StatePTX>,
        esb_timer: IrqTimer<TIMER0>,
        serial: Uarte<UARTE0>,
        timer: Timer<TIMER1>,
        relays: Relays<RollingRtcTimer>,
        rtc: Rtc<RTC0, Started>,
        rtc_timer: RollingRtcTimer,
    }

    #[init]
    fn init(ctx: init::Context) -> init::LateResources {
        let clocks = hal::clocks::Clocks::new(ctx.device.CLOCK);
        let clocks = clocks.enable_ext_hfosc();
        let clocks = clocks.set_lfclk_src_external(LfOscConfiguration::NoExternalNoBypass);
        clocks.start_lfclk();

        let p0 = hal::gpio::p0::Parts::new(ctx.device.P0);

        let uart = ctx.device.UARTE0;
        let mut serial = apply_config!(p0, uart);
        write!(serial, "\r\n--- INIT ---").unwrap();

        static BUFFER: EsbBuffer<U8192, U8192> = EsbBuffer {
            app_to_radio_buf: BBBuffer(ConstBBBuffer::new()),
            radio_to_app_buf: BBBuffer(ConstBBBuffer::new()),
            timer_flag: AtomicBool::new(false),
        };
        let addresses = Addresses::default();
        let config = ConfigBuilder::default()
            .wait_for_ack_timeout(1500)
            .retransmit_delay(2000)
            .maximum_transmit_attempts(3)
            .check()
            .unwrap();
        let (esb_app, esb_irq, esb_timer) = BUFFER
            .try_split(ctx.device.TIMER0, ctx.device.RADIO, addresses, config)
            .unwrap();

        let esb_irq = esb_irq.into_ptx();

        rtt_init_print!();

        if let Some(msg) = panic_persist::get_panic_message_utf8() {
            // write the error message in reasonable chunks
            rprintln!("{}", msg);
        }

        let mut rng = Rng::new(ctx.device.RNG);

        let radio = FleetRadioPtx::new(
            esb_app,
            &[
                0x00, 0x01, 0x02, 0x03, 0x10, 0x11, 0x12, 0x13, 0x20, 0x21, 0x22, 0x23, 0x30, 0x31,
                0x32, 0x33, 0x40, 0x41, 0x42, 0x43, 0x50, 0x51, 0x52, 0x53, 0x60, 0x61, 0x62, 0x63,
                0x70, 0x71, 0x72, 0x73,
            ],
            RollingRtcTimer::new(&RTC_STORE),
            32768 * 2,
            &mut rng,
        );

        let mut rtc = Rtc::new(ctx.device.RTC0);
        rtc.set_prescaler(0).ok();
        rtc.enable_interrupt(RtcInterrupt::Tick, None);
        rtc.enable_event(RtcInterrupt::Tick);
        rtc.get_event_triggered(RtcInterrupt::Tick, true);
        let rtc = rtc.enable_counter();

        // Setup LEDS
        // * 09 - GPIO_30, p10, P0.30
        // * 12 - GPIO_14, p07, P0.14
        // * 11 - GPIO_22, p08, P0.22
        // * 10 - GPIO_31, p09, P0.31
        let relays = Relays::from_pins(
            [
                p0.p0_30
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
                p0.p0_14
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
                p0.p0_22
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
                p0.p0_31
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
            ],
            RollingRtcTimer::new(&RTC_STORE),
        );

        init::LateResources {
            esb_app: radio,
            esb_irq,
            esb_timer,
            serial,
            timer: Timer::new(ctx.device.TIMER1),
            relays,
            rtc,
            rtc_timer: RollingRtcTimer::new(&RTC_STORE),
        }
    }

    #[idle(resources = [serial, esb_app, timer, relays])]
    fn idle(ctx: idle::Context) -> ! {
        let esb_app = ctx.resources.esb_app;
        let timer = ctx.resources.timer;
        let relays = ctx.resources.relays;

        let msg = DeviceToHost::General(GeneralDeviceMessage::InitializeSession);

        loop {
            NVIC::pend(hal::target::Interrupt::RTC0);
            match esb_app.send(&msg, 0) {
                Ok(_) => rprintln!("Sent {:?}", msg),
                Err(e) => rprintln!("Send err: {:?}", e),
            };
            timer.delay_ms(250u8);

            use fleet_icd::radio::{HostToDevice, PlantLightHostMessage};

            'rx: loop {
                match esb_app.receive() {
                    Ok(None) => {
                        break 'rx;
                    }
                    Ok(Some(RxMessage {
                        msg:
                            HostToDevice::PlantLight(PlantLightHostMessage::SetRelay { relay, state }),
                        ..
                    })) => {
                        relays.set_relay(relay, state).ok();
                    }
                    Ok(Some(m)) => {
                        rprintln!("Got msg: {:?}", m);
                    }
                    Err(e) => {
                        rprintln!("RxErr: {:?}", e);
                    }
                }
            }
        }
    }

    #[task(binds = RTC0, resources = [rtc, rtc_timer], priority = 1)]
    fn rtc_tick(ctx: rtc_tick::Context) {
        // Check and clear interrupt
        ctx.resources
            .rtc
            .get_event_triggered(RtcInterrupt::Tick, true);
        ctx.resources.rtc_timer.tick();
    }

    #[task(binds = RADIO, resources = [esb_irq], priority = 3)]
    fn radio_interrupt(ctx: radio_interrupt::Context) {
        match ctx.resources.esb_irq.radio_interrupt() {
            Err(Error::MaximumAttempts) => {
                ATTEMPTS_FLAG.store(true, Ordering::Release);
            }
            Err(e) => panic!("Found error {:?}", e),
            Ok(_state) => {} //rprintln!("{:?}", _state),
        }
    }

    #[task(binds = TIMER0, resources = [esb_timer], priority = 3)]
    fn timer0(ctx: timer0::Context) {
        ctx.resources.esb_timer.timer_interrupt();
    }
};

use panic_persist;
