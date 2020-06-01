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
    core::{
        default::Default,
        fmt::Write,
        sync::atomic::{AtomicBool, Ordering, AtomicU32},
    },
    cortex_m::asm::bkpt,
    esb::{
        consts::*, irq::StatePTX, Addresses, BBBuffer, ConfigBuilder, ConstBBBuffer, Error,
        EsbBuffer, EsbIrq, IrqTimer,
    },
    hal::{
        gpio::Level,
        pac::{TIMER0, TIMER1, UARTE0},
        uarte::{Baudrate, Parity, Uarte},
        Rng, Timer,
    },
    rtt_target::{rtt_init_print, rprintln},
};

use fleet_esb::{
    ptx::FleetRadioPtx,
    RollingTimer,
};

use fleet_icd::radio::{
    DeviceToHost,
    HostToDevice,
    GeneralDeviceMessage,
};

use embedded_hal::blocking::delay::DelayMs;

static FAKE_TIME: AtomicU32 = AtomicU32::new(0);

pub struct FakeClock {
    clock: &'static AtomicU32,
}

impl RollingTimer for FakeClock {
    fn get_current_tick(&self) -> u32 {
        self.clock.fetch_add(1, Ordering::SeqCst)
    }
}

static ATTEMPTS_FLAG: AtomicBool = AtomicBool::new(false);

#[rtfm::app(device = crate::hal::pac, peripherals = true)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPtx<U8192, U8192, DeviceToHost, HostToDevice, FakeClock>,
        esb_irq: EsbIrq<U8192, U8192, TIMER0, StatePTX>,
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
        write!(serial, "\r\n--- INIT ---").unwrap();

        static BUFFER: EsbBuffer<U8192, U8192> = EsbBuffer {
            app_to_radio_buf: BBBuffer(ConstBBBuffer::new()),
            radio_to_app_buf: BBBuffer(ConstBBBuffer::new()),
            timer_flag: AtomicBool::new(false),
        };
        let addresses = Addresses::default();
        let config = ConfigBuilder::default()
            .maximum_transmit_attempts(0)
            .check()
            .unwrap();
        let (esb_app, esb_irq, esb_timer) = BUFFER
            .try_split(ctx.device.TIMER0, ctx.device.RADIO, addresses, config)
            .unwrap();

        let esb_irq = esb_irq.into_ptx();

        rtt_init_print!();

        if let Some(msg) = panic_persist::get_panic_message_bytes() {
            // write the error message in reasonable chunks
            for i in msg.chunks(255) {
                let _ = serial.write(i);
            }
            bkpt();
        }

        let mut rng = Rng::new(ctx.device.RNG);

        let radio = FleetRadioPtx::new(
            esb_app,
            &[
                0x00, 0x01, 0x02, 0x03,
                0x10, 0x11, 0x12, 0x13,
                0x20, 0x21, 0x22, 0x23,
                0x30, 0x31, 0x32, 0x33,
                0x40, 0x41, 0x42, 0x43,
                0x50, 0x51, 0x52, 0x53,
                0x60, 0x61, 0x62, 0x63,
                0x70, 0x71, 0x72, 0x73,
            ],
            FakeClock { clock: &FAKE_TIME },
            100,
            &mut rng,
        );

        init::LateResources {
            esb_app: radio,
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

        let msg = DeviceToHost::General(GeneralDeviceMessage::InitializeSession);

        loop {
            match esb_app.send(&msg) {
                Ok(_) => rprintln!("Sent {:?}", msg),
                Err(e) => rprintln!("Send err: {:?}", e),
            };
            timer.delay_ms(250u8);

            'rx: loop {
                match esb_app.receive() {
                    Ok(None) => {
                        break 'rx;
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

// #[inline(never)]
// #[panic_handler]
// fn panic(info: &PanicInfo) -> ! {
//     rprintln!("{}", info);
//     loop {
//         compiler_fence(Ordering::SeqCst);
//     }
// }
