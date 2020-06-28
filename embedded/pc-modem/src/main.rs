#![no_std]
#![no_main]

// Import the right HAL/PAC crate, depending on the target chip
use nrf52832_hal as hal;

use {
    core::{default::Default, sync::atomic::AtomicBool},
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
use embedded_hal::blocking::delay::DelayMs;

use fleet_esb::{
    prx::FleetRadioPrx,
    RxMessage,
};
use fleet_icd::{
    radio::{DeviceToHost, HostToDevice, GeneralDeviceMessage},
    modem::{PcToModem, ModemToPc},
    Buffer as CobsBuffer, FeedResult,
};
use fleet_keys::keys::KEY;

use fleet_uarte;

use postcard::to_slice_cobs;

// Panic provider crate
use panic_persist;

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

        cobs_buf: CobsBuffer<U256>,
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
        let mut esb_irq = esb_irq.into_prx();
        esb_irq.start_receiving().unwrap();

        static UBUF: fleet_uarte::buffer::UarteBuffer<U1024, U1024> =
            fleet_uarte::buffer::UarteBuffer {
                txd_buf: BBBuffer(ConstBBBuffer::new()),
                rxd_buf: BBBuffer(ConstBBBuffer::new()),
                timeout_flag: AtomicBool::new(false),
            };

        rtt_init_print!();

        if let Some(msg) = panic_persist::get_panic_message_utf8() {
            rprintln!("panic: {}", msg);
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
                255,
                50_000,
            )
            .unwrap();

        init::LateResources {
            esb_app,
            esb_irq,
            esb_timer,
            timer: Timer::new(ctx.device.TIMER1),
            uarte_timer: ue.timer,
            uarte_irq: ue.irq,
            uarte_app: ue.app,
            cobs_buf: CobsBuffer::new(),
        }
    }

    #[idle(resources = [esb_app, timer, uarte_app, cobs_buf])]
    fn idle(ctx: idle::Context) -> ! {
        let esb_app = ctx.resources.esb_app;
        let timer = ctx.resources.timer;
        let uarte_app = ctx.resources.uarte_app;
        let cobs_buf = ctx.resources.cobs_buf;

        rprintln!("Start!");

        loop {
            // Check for radio messages
            match esb_app.receive() {
                Ok(Some(RxMessage { msg: DeviceToHost::General(GeneralDeviceMessage::InitializeSession), .. })) => {
                    // Ignore
                }
                Ok(Some(msg)) => {
                    match uarte_app.write_grant(128) {
                        Ok(mut wgr) => {
                            let smsg = ModemToPc::Incoming {
                                pipe: msg.meta.pipe,
                                msg: msg.msg,
                            };

                            let used: usize = to_slice_cobs(&smsg, &mut wgr)
                                .map(|buf| buf.len())
                                .unwrap_or(0);

                            wgr.commit(used);
                        }
                        Err(e) => {
                            rprintln!("uartetxerr: {:?}", e);
                        }
                    }
                }
                Ok(None) => {},
                Err(e) => {
                    rprintln!("rxerr: {:?}", e);
                }
            }

            // Check for uart messages
            if let Ok(rgr) = uarte_app.read() {
                let mut buf: &[u8] = &rgr;
                loop {
                    if buf.is_empty() {
                        break;
                    }
                    match cobs_buf.feed::<PcToModem>(buf) {
                        // Consumed all data, still pending
                        FeedResult::Consumed => break,

                        // Buffer was filled. Contains remaining section of input, if any
                        FeedResult::OverFull(new_buf) => {
                            rprintln!("Overfull");
                            buf = new_buf;
                        }

                        // Reached end of chunk, but deserialization failed. Contains
                        // remaining section of input, if any
                        FeedResult::DeserError(new_buf) => {
                            rprintln!("Deser Error");
                            buf = new_buf;
                        }

                        // Deserialization complete. Contains deserialized data and
                        // remaining section of input, if any
                        FeedResult::Success {
                            data,
                            remaining
                        } => {
                            match data {
                                PcToModem::Outgoing { msg, pipe } => {
                                    rprintln!("sending to {}: {:?}", pipe, msg);
                                    esb_app.send(&msg, pipe).unwrap();
                                }
                            }

                            buf = remaining;
                        }
                    }
                }

                let len = rgr.len();
                rgr.release(len);
            }
            timer.delay_ms(10u8);
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
        ctx.resources.uarte_timer.interrupt();
    }

    #[task(binds = UARTE0_UART0, resources = [uarte_irq])]
    fn uarte0(ctx: uarte0::Context) {
        ctx.resources.uarte_irq.interrupt();
    }
};
