#![no_std]
#![no_main]

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
        pac::{TIMER0, TIMER1, TIMER2},
        ppi::{Parts, Ppi0},
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

use fleet_uarte;

// Panic provider crate
use panic_persist as _;

#[rtfm::app(device = crate::hal::pac, peripherals = true)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPrx<U8192, U8192, HostToDevice, DeviceToHost>,
        esb_irq: EsbIrq<U8192, U8192, TIMER0, StatePRX>,
        esb_timer: IrqTimer<TIMER0>,
        timer: Timer<TIMER1>,

        uarte_timer: fleet_uarte::irq::UarteTimer<TIMER2>,
        uarte_irq: fleet_uarte::irq::UarteIrq<U1024, U1024, Ppi0>,
        uarte_app: fleet_uarte::app::UarteApp<U1024, U1024>,
    }

    #[init]
    fn init(ctx: init::Context) -> init::LateResources {
        let _clocks = hal::clocks::Clocks::new(ctx.device.CLOCK).enable_ext_hfosc();

        let p0 = hal::gpio::p0::Parts::new(ctx.device.P0);

        let uart = ctx.device.UARTE0;

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
        let esb_irq = esb_irq.into_prx();
        // esb_irq.start_receiving().unwrap();

        static UBUF: fleet_uarte::buffer::UarteBuffer<U1024, U1024> =
            fleet_uarte::buffer::UarteBuffer {
                txd_buf: BBBuffer(ConstBBBuffer::new()),
                rxd_buf: BBBuffer(ConstBBBuffer::new()),
                timeout_flag: AtomicBool::new(false),
            };

        rtt_init_print!();

        if let Some(msg) = panic_persist::get_panic_message_bytes() {
            // write the error message in reasonable chunks
            for i in msg.chunks(255) {
                let _ = i;
            }
            bkpt();
        }

        let esb_app = FleetRadioPrx::new(esb_app, KEY.key());

        let rxd = p0.p0_11.into_floating_input().degrade();
        let txd = p0.p0_05.into_push_pull_output(Level::Low).degrade();

        let ppi_channels = Parts::new(ctx.device.PPI);
        let channel0 = ppi_channels.ppi0;

        let uarte_pins = hal::uarte::Pins {
            rxd,
            txd,
            cts: None,
            rts: None,
        };

        let ue = UBUF
            .try_split(
                uarte_pins,
                hal::uarte::Parity::EXCLUDED,
                hal::uarte::Baudrate::BAUD230400,
                ctx.device.TIMER2,
                channel0,
                uart,
                32,
            )
            .unwrap();

        // ctx.device.PPI.chenset.modify(|_r, w| w.ch0().set_bit());

        init::LateResources {
            esb_app,
            esb_irq,
            esb_timer,
            timer: Timer::new(ctx.device.TIMER1),
            uarte_timer: ue.timer,
            uarte_irq: ue.irq,
            uarte_app: ue.app,
        }
    }

    #[idle(resources = [esb_app, timer, uarte_app])]
    fn idle(ctx: idle::Context) -> ! {
        let esb_app = ctx.resources.esb_app;
        let timer = ctx.resources.timer;
        let uarte_app = ctx.resources.uarte_app;

        let on = true;

        use embedded_hal::timer::CountDown;

        rprintln!("Start!");

        timer.start(5_000_000u32);

        loop {
            if let Ok(rgr) = uarte_app.read() {
                let len = rgr.len();
                rprintln!("Brr: {}", len);
                if let Ok(mut wgr) = uarte_app.write_grant(len) {
                    wgr.copy_from_slice(&rgr);
                    wgr.commit(len);
                }
                rgr.release(len);
            }
            if timer.wait().is_ok() {
                rprintln!("Hello from idle!");
                timer.start(5_000_000u32);
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

    #[task(binds = TIMER2, resources = [uarte_timer])]
    fn timer2(ctx: timer2::Context) {
        // rprintln!("Hello from timer2!");
        ctx.resources.uarte_timer.interrupt();
    }

    #[task(binds = UARTE0_UART0, resources = [uarte_irq])]
    fn uarte0(ctx: uarte0::Context) {
        // rprintln!("Hello from uarte0!");
        ctx.resources.uarte_irq.interrupt();
    }
};
