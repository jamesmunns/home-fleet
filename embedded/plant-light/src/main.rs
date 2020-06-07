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
        sync::atomic::{AtomicBool, Ordering},
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
        uarte::{Baudrate, Parity, Uarte},
        Rng, Rtc, Timer,
    },
    rtt_target::{rprintln, rtt_init_print},
};

use fleet_esb::{ptx::FleetRadioPtx, RxMessage};

use fleet_icd::radio::{
    DeviceToHost, GeneralDeviceMessage, HostToDevice, PlantLightDeviceMessage,
    PlantLightHostMessage,
};

use fleet_keys::keys::KEY;

use embedded_hal::blocking::delay::DelayMs;

use relays::Relays;
use timer::RollingRtcTimer;

static ATTEMPTS_FLAG: AtomicBool = AtomicBool::new(false);

#[rtfm::app(device = crate::hal::pac, peripherals = true, monotonic = crate::timer::RollingRtcTimer)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPtx<U2048, U2048, DeviceToHost, HostToDevice, RollingRtcTimer>,
        esb_irq: EsbIrq<U2048, U2048, TIMER0, StatePTX>,
        esb_timer: IrqTimer<TIMER0>,
        serial: Uarte<UARTE0>,
        timer: Timer<TIMER1>,
        relays: Relays<RollingRtcTimer>,
        rtc: Rtc<RTC0, Started>,
        rtc_timer: RollingRtcTimer,
        rtc_timer2: RollingRtcTimer,
    }

    #[init(spawn = [relay_periodic, rx_periodic, relay_status])]
    fn init(ctx: init::Context) -> init::LateResources {
        let clocks = hal::clocks::Clocks::new(ctx.device.CLOCK);
        let clocks = clocks.enable_ext_hfosc();
        let clocks = clocks.set_lfclk_src_external(LfOscConfiguration::NoExternalNoBypass);
        clocks.start_lfclk();

        let p0 = hal::gpio::p0::Parts::new(ctx.device.P0);

        let uart = ctx.device.UARTE0;
        let mut serial = apply_config!(p0, uart);
        write!(serial, "\r\n--- INIT ---").unwrap();

        static BUFFER: EsbBuffer<U2048, U2048> = EsbBuffer {
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
            KEY.key(),
            RollingRtcTimer::new(),
            timer::TICKS_PER_SECOND * 2,
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
                p0.p0_15
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
                p0.p0_08
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
                p0.p0_07
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
                p0.p0_04
                    .into_open_drain_output(OpenDrainConfig::HighDrive0Disconnect1, Level::High)
                    .degrade(),
            ],
            RollingRtcTimer::new(),
        );

        rprintln!("Pre Spawn");

        ctx.spawn.rx_periodic().ok();
        ctx.spawn.relay_periodic().ok();
        ctx.spawn.relay_status().ok();

        rprintln!("Post Spawn");

        init::LateResources {
            esb_app: radio,
            esb_irq,
            esb_timer,
            serial,
            timer: Timer::new(ctx.device.TIMER1),
            relays,
            rtc,
            rtc_timer: RollingRtcTimer::new(),
            rtc_timer2: RollingRtcTimer::new(),
        }
    }

    #[idle(resources = [timer, rtc_timer2])]
    fn idle(ctx: idle::Context) -> ! {
        rprintln!("Still alive!");
        loop {
            ctx.resources.timer.delay_ms(5000u16);
            rprintln!("Still alive!");
        }
    }

    #[task(binds = RTC0, resources = [rtc, rtc_timer], priority = 2)]
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

    #[task(schedule = [relay_periodic], resources = [relays])]
    fn relay_periodic(ctx: relay_periodic::Context) {
        rprintln!("Enter relay_per");
        ctx.resources.relays.check_timeout();
        ctx.schedule
            .relay_periodic(ctx.scheduled + timer::SIGNED_TICKS_PER_SECOND)
            .ok();
    }

    #[task(schedule = [relay_status], resources = [relays, esb_app])]
    fn relay_status(ctx: relay_status::Context) {
        const INTERVAL: i32 = timer::SIGNED_TICKS_PER_SECOND * 3;

        let stat = ctx.resources.relays.current_state();
        let msg = DeviceToHost::PlantLight(PlantLightDeviceMessage::Status(stat));
        ctx.resources.esb_app.send(&msg, 0).ok();

        ctx.schedule.relay_status(ctx.scheduled + INTERVAL).ok();
    }

    #[task(schedule = [rx_periodic], spawn = [relay_command], resources = [esb_app])]
    fn rx_periodic(ctx: rx_periodic::Context) {
        // Roughly 10ms
        const INTERVAL: i32 = timer::SIGNED_TICKS_PER_SECOND / 100;
        // Roughly 100ms
        const POLL_PRX_INTERVAL: u32 = timer::TICKS_PER_SECOND / 10;
        const RESET_INTERVAL: u32 = timer::TICKS_PER_SECOND * 60;

        let esb_app = ctx.resources.esb_app;

        'rx: loop {
            match esb_app.receive() {
                Ok(None) => break 'rx,
                Ok(Some(RxMessage {
                    msg: HostToDevice::PlantLight(m),
                    ..
                })) => match ctx.spawn.relay_command(m) {
                    Ok(_) => {}
                    Err(e) => rprintln!("spawn err: {:?}", e),
                },
                Ok(Some(m)) => {
                    rprintln!("Got unproc'd msg: {:?}", m);
                }
                Err(e) => {
                    rprintln!("RxErr: {:?}", e);
                }
            }
        }

        if esb_app.ticks_since_last_tx() > POLL_PRX_INTERVAL {
            let msg = DeviceToHost::General(GeneralDeviceMessage::InitializeSession);

            match esb_app.send(&msg, 0) {
                Ok(_) => { /*rprintln!("Sent {:?}", msg) */ }
                Err(e) => rprintln!("Send err: {:?}", e),
            }
        }

        // Decide if we should watchdog
        if esb_app.ticks_since_last_rx() > RESET_INTERVAL {
            panic!("It's quiet, tooooo quiet.");
        }

        ctx.schedule.rx_periodic(ctx.scheduled + INTERVAL).ok();
    }

    #[task(resources = [relays], capacity = 5)]
    fn relay_command(ctx: relay_command::Context, cmd: PlantLightHostMessage) {
        rprintln!("Enter relay_cmd");

        if let PlantLightHostMessage::SetRelay { relay, state } = cmd {
            ctx.resources.relays.set_relay(relay, state).ok();
        }
    }

    // Sacrificial hardware interrupts
    extern "C" {
        fn SWI1_EGU1();
    // fn SWI2_EGU2();
    // fn SWI3_EGU3();
    }
};

use panic_persist;
