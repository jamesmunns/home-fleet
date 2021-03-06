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
    core::{default::Default, fmt::Write, sync::atomic::AtomicBool},
    cortex_m::asm::bkpt,
    esb::{
        consts::*, irq::StatePRX, Addresses, BBBuffer, Config, ConstBBBuffer, Error, EsbBuffer,
        EsbIrq, IrqTimer,
    },
    hal::{
        gpio::Level,
        pac::{TIMER0, TIMER1},
        Timer,
    },
    rtt_target::{rprintln, rtt_init_print},
};

#[cfg(not(feature = "51"))]
use hal::{
    pac::UARTE0,
    uarte::{Baudrate, Parity, Uarte},
};

use fleet_esb::prx::FleetRadioPrx;
use fleet_icd::radio::{DeviceToHost, HostToDevice, PlantLightHostMessage, RelayIdx, RelayState};
use fleet_keys::keys::KEY;

// Panic provider crate
use panic_persist as _;

#[rtfm::app(device = crate::hal::pac, peripherals = true)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPrx<U8192, U8192, HostToDevice, DeviceToHost>,
        esb_irq: EsbIrq<U8192, U8192, TIMER0, StatePRX>,
        esb_timer: IrqTimer<TIMER0>,
        serial: Uarte<UARTE0>,
        timer: Timer<TIMER1>,
    }

    #[init]
    fn init(ctx: init::Context) -> init::LateResources {
        let _clocks = hal::clocks::Clocks::new(ctx.device.CLOCK).enable_ext_hfosc();

        let p0 = hal::gpio::p0::Parts::new(ctx.device.P0);

        let uart = ctx.device.UARTE0;
        let mut serial = apply_config!(p0, uart);
        writeln!(serial, "\r\n--- INIT ---").unwrap();

        static BUFFER: EsbBuffer<U8192, U8192> = EsbBuffer {
            app_to_radio_buf: BBBuffer(ConstBBBuffer::new()),
            radio_to_app_buf: BBBuffer(ConstBBBuffer::new()),
            timer_flag: AtomicBool::new(false),
        };
        let addresses = Addresses::default();
        let config = Config::default();
        let (esb_app, esb_irq, esb_timer) = BUFFER
            .try_split(ctx.device.TIMER0, ctx.device.RADIO, addresses, config)
            .unwrap();
        let mut esb_irq = esb_irq.into_prx();
        esb_irq.start_receiving().unwrap();

        rtt_init_print!();

        if let Some(msg) = panic_persist::get_panic_message_bytes() {
            // write the error message in reasonable chunks
            for i in msg.chunks(255) {
                let _ = serial.write(i);
            }
            bkpt();
        }

        let esb_app = FleetRadioPrx::new(esb_app, KEY.key());

        init::LateResources {
            esb_app,
            esb_irq,
            esb_timer,
            serial,
            timer: Timer::new(ctx.device.TIMER1),
        }
    }

    #[idle(resources = [serial, esb_app, timer])]
    fn idle(ctx: idle::Context) -> ! {
        let esb_app = ctx.resources.esb_app;
        let timer = ctx.resources.timer;

        let on = true;

        use embedded_hal::timer::CountDown;

        timer.start(5_000_000u32);

        loop {
            use fleet_esb::RxMessage;
            use fleet_icd::radio::GeneralDeviceMessage;

            match esb_app.receive() {
                Ok(None) => {}
                Ok(Some(RxMessage {
                    msg: DeviceToHost::General(GeneralDeviceMessage::InitializeSession),
                    ..
                })) => {}
                Ok(Some(m)) => {
                    rprintln!("Got {:#?}", m);
                }
                Err(e) => {
                    rprintln!("RxErr: {:?}", e);
                }
            }

            if timer.wait().is_ok() {
                timer.start(6_000_000u32);
                let state = if on { RelayState::On } else { RelayState::Off };
                // on = !on;

                for relay in [
                    RelayIdx::Relay0,
                    RelayIdx::Relay1,
                    RelayIdx::Relay2,
                    RelayIdx::Relay3,
                ]
                .iter()
                {
                    let resp = HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: *relay,
                        state,
                    });
                    match esb_app.send(&resp, 0) {
                        Ok(_) => rprintln!("Sent {:?}", resp),
                        Err(e) => rprintln!("TxErr: {:?}", e),
                    }
                }
            }
        }
    }

    #[task(binds = RADIO, resources = [esb_irq], priority = 3)]
    fn radio(ctx: radio::Context) {
        match ctx.resources.esb_irq.radio_interrupt() {
            Err(Error::MaximumAttempts) => {}
            Err(e) => {
                bkpt();
                panic!("Found error {:?}", e);
            }
            Ok(_state) => {} //rprintln!("{:?}", state).unwrap(),
        }
    }

    #[task(binds = TIMER0, resources = [esb_timer], priority = 3)]
    fn timer0(ctx: timer0::Context) {
        ctx.resources.esb_timer.timer_interrupt();
    }
};
