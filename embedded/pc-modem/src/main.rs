#![no_std]
#![no_main]

// Import the right HAL/PAC crate, depending on the target chip
use nrf52832_hal as hal;

use {
    core::{default::Default, sync::atomic::AtomicBool},
    cortex_m::{asm::bkpt, peripheral::SCB},
    cortex_m_rt::exception,
    esb::{
        consts::*, irq::StatePRX, Addresses, BBBuffer, ConfigBuilder, ConstBBBuffer, Error, EsbApp,
        EsbBuffer, EsbHeader, EsbIrq, IrqTimer, TxPower,
    },
    hal::{
        clocks::LfOscConfiguration,
        gpio::{Level, Output, Pin, PushPull},
        pac::{RTC0, TIMER0, TIMER2},
        ppi::{Parts, Ppi0},
        rtc::{Rtc, RtcInterrupt, Started},
        wdt::{count, handles::HdlN, Parts as WatchdogParts, Watchdog, WatchdogHandle},
    },
    rtt_target::{rprintln, rtt_init_print},
};

use anachro_icd::{arbitrator::Arbitrator, component::Component, Uuid};
use anachro_server::{Request, Response};
use fleet_esb::{prx::FleetRadioPrx, BorrowRxMessage, RxMessage};
use fleet_icd::{Buffer as CobsBuffer, WithResult};
use fleet_keys::keys::KEY;

use fleet_uarte;

use postcard::{from_bytes, to_slice, to_slice_cobs};

mod timer;

use timer::RollingRtcTimer;

use blinq::{consts, patterns, Blinq};

// Panic provider crate
use panic_persist;

static BUFFER: EsbBuffer<U8192, U8192> = EsbBuffer {
    app_to_radio_buf: BBBuffer(ConstBBBuffer::new()),
    radio_to_app_buf: BBBuffer(ConstBBBuffer::new()),
    timer_flag: AtomicBool::new(false),
};

#[rtic::app(device = crate::hal::pac, peripherals = true, monotonic = crate::timer::RollingRtcTimer)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPrx<U8192, U8192>,
        esb_irq: EsbIrq<U8192, U8192, TIMER0, StatePRX>,
        esb_timer: IrqTimer<TIMER0>,
        esb_wdog: WatchdogHandle<HdlN>,

        uarte_timer: fleet_uarte::irq::UarteTimer<TIMER2>,
        uarte_irq: fleet_uarte::irq::UarteIrq<U1024, U1024, Ppi0>,
        uarte_app: fleet_uarte::app::UarteApp<U1024, U1024>,
        uarte_wdog: WatchdogHandle<HdlN>,

        cobs_buf: CobsBuffer<U256>,

        rtc: Rtc<RTC0, Started>,
        rtc_timer: RollingRtcTimer,

        blinq0: Blinq<consts::U8, Pin<Output<PushPull>>>,
        blinq1: Blinq<consts::U8, Pin<Output<PushPull>>>,
        blinq2: Blinq<consts::U8, Pin<Output<PushPull>>>,
        blinq3: Blinq<consts::U8, Pin<Output<PushPull>>>,
    }

    #[init(spawn = [led_periodic])]
    fn init(ctx: init::Context) -> init::LateResources {
        let clocks = hal::clocks::Clocks::new(ctx.device.CLOCK);
        let clocks = clocks.enable_ext_hfosc();
        let clocks = clocks.set_lfclk_src_external(LfOscConfiguration::NoExternalNoBypass);
        clocks.start_lfclk();

        let p0 = hal::gpio::p0::Parts::new(ctx.device.P0);

        let uart = ctx.device.UARTE0;

        let addresses = Addresses::new(
            [0xE7, 0xE7, 0xE7, 0xE7], // default
            [0xC2, 0xC2, 0xC2, 0xC2], // default
            [0xE7, 0xC2, 0xC3, 0xC4], // default
            [0xC5, 0xC6, 0xC7, 0xC8], // default
            8,                        // default: 2
        )
        .unwrap();

        let config = ConfigBuilder::default()
            .tx_power(TxPower::POS4DBM)
            .check()
            .unwrap();

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

        // Create a new watchdog instance
        //
        // In case the watchdog is already running, just spin and let it expire, since
        // we can't configure it anyway. This usually happens when we first program
        // the device and the watchdog was previously active
        let (uarte_wdog, esb_wdog) = match Watchdog::try_new(ctx.device.WDT) {
            Ok(mut watchdog) => {
                // Set the watchdog to timeout after 60 seconds (in 32.768kHz ticks)
                watchdog.set_lfosc_ticks(60 * 32768);

                // Activate the watchdog with four handles
                let WatchdogParts {
                    watchdog: _watchdog,
                    handles,
                } = watchdog.activate::<count::Two>();

                handles
            }
            Err(wdt) => match Watchdog::try_recover::<count::Two>(wdt) {
                Ok(WatchdogParts { mut handles, .. }) => {
                    rprintln!("Oops, watchdog already active, but recovering!");

                    // Pet all the dogs quickly to reset to default timeout
                    handles.0.pet();
                    handles.1.pet();

                    handles
                }
                Err(_wdt) => {
                    rprintln!("Oops, watchdog already active, resetting!");
                    loop {
                        continue;
                    }
                }
            },
        };

        if ctx.device.POWER.resetreas.read().dog().is_detected() {
            ctx.device.POWER.resetreas.modify(|_r, w| {
                // Clear the watchdog reset reason bit
                w.dog().set_bit()
            });
            rprintln!("Restarted by the dog!");
        } else {
            rprintln!("Not restarted by the dog!");
        }

        let (uarte_wdog, esb_wdog) = (uarte_wdog.degrade(), esb_wdog.degrade());

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

        let mut rtc = Rtc::new(ctx.device.RTC0);
        rtc.set_prescaler(0).ok();
        rtc.enable_interrupt(RtcInterrupt::Tick, None);
        rtc.enable_event(RtcInterrupt::Tick);
        rtc.get_event_triggered(RtcInterrupt::Tick, true);
        let rtc = rtc.enable_counter();

        // D9 : Led::new(pins.p0_30.degrade()),
        // D12: Led::new(pins.p0_14.degrade()),
        // D11: Led::new(pins.p0_22.degrade()),
        // D10: Led::new(pins.p0_31.degrade()),

        let mut blinq0 = Blinq::new(p0.p0_30.into_push_pull_output(Level::High).degrade(), true);
        let mut blinq1 = Blinq::new(p0.p0_14.into_push_pull_output(Level::High).degrade(), true);
        let mut blinq2 = Blinq::new(p0.p0_22.into_push_pull_output(Level::High).degrade(), true);
        let mut blinq3 = Blinq::new(p0.p0_31.into_push_pull_output(Level::High).degrade(), true);

        // Insert 3s of all white short blink on reset
        for _ in 0..3 {
            blinq0.enqueue(patterns::blinks::QUARTER_DUTY);
            blinq1.enqueue(patterns::blinks::QUARTER_DUTY);
            blinq2.enqueue(patterns::blinks::QUARTER_DUTY);
            blinq3.enqueue(patterns::blinks::QUARTER_DUTY);
        }

        ctx.spawn.led_periodic().ok();

        init::LateResources {
            esb_app,
            esb_irq,
            esb_timer,
            esb_wdog,
            uarte_timer: ue.timer,
            uarte_irq: ue.irq,
            uarte_app: ue.app,
            uarte_wdog,
            cobs_buf: CobsBuffer::new(),
            rtc,
            rtc_timer: RollingRtcTimer::new(),

            blinq0,
            blinq1,
            blinq2,
            blinq3,
        }
    }

    #[idle(resources = [esb_app, uarte_app, cobs_buf, esb_wdog, uarte_wdog, blinq0, blinq1, blinq2, blinq3])]
    fn idle(mut ctx: idle::Context) -> ! {
        let esb_app = ctx.resources.esb_app;
        let uarte_app = ctx.resources.uarte_app;
        let cobs_buf = ctx.resources.cobs_buf;
        let uarte_wdog = ctx.resources.uarte_wdog;
        let esb_wdog = ctx.resources.esb_wdog;
        let mut blinq2 = ctx.resources.blinq2;

        let mut broker = anachro_server::Broker::default();

        let uarte_uuid = Uuid::from_bytes([
            0x01, 0x02, 0x03, 0x04, 0x01, 0x02, 0x03, 0x04, 0x01, 0x02, 0x03, 0x04, 0x01, 0x02,
            0x03, 0x04,
        ]);

        let pipe0_uuid = Uuid::from_bytes([
            0x04, 0x04, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]);

        rprintln!("Start!");

        broker.register_client(&uarte_uuid).unwrap();
        broker.register_client(&pipe0_uuid).unwrap();

        loop {
            let rx = esb_app.receive_with();

            if let Ok(mut msg) = rx {
                let _ = msg.view_with(|msg: BorrowRxMessage<Component>| {
                    let source = match msg.meta.pipe {
                        0 => pipe0_uuid.clone(),
                        i => {
                            rprintln!("pipe {}?", i);
                            return;
                        }
                    };

                    // Decoded a wireless message - pet the dog
                    esb_wdog.pet();

                    rprintln!("Processing {:?}", msg.msg);

                    match broker.process_msg(&Request {
                        source,
                        msg: msg.msg,
                    }) {
                        Ok(responses) => {
                            for resp in responses.iter() {
                                match resp.dest {
                                    x if x == pipe0_uuid => {
                                        esb_app.send(&resp.msg, 0).ok();
                                    }
                                    x if x == uarte_uuid => {
                                        try_send(uarte_app, &resp.msg).ok();
                                    }
                                    _ => {
                                        rprintln!("WHO DAT");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            rprintln!("Process error: {:?}", e);
                        }
                    }
                });

                // TODO
                msg.fgr.release();
            }

            // Check for uart messages
            if let Ok(rgr) = uarte_app.read() {
                let mut buf: &[u8] = &rgr;
                loop {
                    if buf.is_empty() {
                        break;
                    }
                    match cobs_buf.feed_with(buf, |msg: Component| {
                        rprintln!("From the UARTE: {:?}", msg);
                        if let Ok(msgs) = broker.process_msg(&Request {
                            msg,
                            source: uarte_uuid.clone(),
                        }) {
                            rprintln!("Got {} messages in return", msgs.len());
                            for msg in msgs {
                                if msg.dest == uarte_uuid {
                                    rprintln!("TO THE UARTE: {:?};{:?}", msg.dest, msg.msg);
                                    // Send it to uarte
                                    if try_send(uarte_app, &msg.msg).is_err() {
                                        blinq2.lock(|b| {
                                            b.enqueue(patterns::blinks::LONG_ON_OFF);
                                        });
                                    }
                                } else if msg.dest == pipe0_uuid {
                                    rprintln!("TO THE RADIO: {:?};{:?}", msg.dest, msg.msg);
                                    esb_app.send(&msg.msg, 0).ok();
                                } else {
                                    rprintln!("TO ???");
                                }
                            }
                        } else {
                            rprintln!("broker said :(");
                        }
                    }) {
                        // Consumed all data, still pending
                        WithResult::Consumed => {
                            ctx.resources.blinq1.lock(|b| {
                                b.enqueue(patterns::blinks::MEDIUM_ON_OFF);
                            });
                            break;
                        }

                        // Buffer was filled. Contains remaining section of input, if any
                        WithResult::OverFull(new_buf) => {
                            ctx.resources.blinq1.lock(|b| {
                                b.enqueue(patterns::blinks::SHORT_ON_OFF);
                            });
                            rprintln!("Overfull");
                            buf = new_buf;
                        }

                        // Reached end of chunk, but deserialization failed. Contains
                        // remaining section of input, if any
                        WithResult::DeserError(new_buf) => {
                            ctx.resources.blinq1.lock(|b| {
                                b.enqueue(patterns::blinks::SHORT_ON_OFF);
                            });
                            rprintln!("Deser Error");
                            buf = new_buf;
                        }

                        // Deserialization complete. Contains deserialized data and
                        // remaining section of input, if any
                        WithResult::SuccessWith { result, remaining } => {
                            ctx.resources.blinq1.lock(|b| {
                                b.enqueue(patterns::blinks::LONG_ON_OFF);
                            });
                            // On successful decode, pet the watchdog
                            uarte_wdog.pet();

                            rprintln!("Success said: {:?}", result);

                            buf = remaining;
                        }
                    }
                }

                let len = rgr.len();
                rgr.release(len);
            }
        }
    }

    #[task(resources = [blinq0, blinq1, blinq2, blinq3], schedule = [led_periodic])]
    fn led_periodic(ctx: led_periodic::Context) {
        ctx.resources.blinq0.step();
        ctx.resources.blinq1.step();
        ctx.resources.blinq2.step();
        ctx.resources.blinq3.step();

        ctx.schedule
            .led_periodic(ctx.scheduled + (timer::SIGNED_TICKS_PER_SECOND / 4))
            .ok();
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

    /// This event fires every time the hardware RTC ticks
    ///
    /// This also feeds the semi-global Rolling RTC Timer
    #[task(binds = RTC0, resources = [rtc, rtc_timer], priority = 2)]
    fn rtc_tick(ctx: rtc_tick::Context) {
        // Check and clear interrupt
        ctx.resources
            .rtc
            .get_event_triggered(RtcInterrupt::Tick, true);
        ctx.resources.rtc_timer.tick();
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

    // Sacrificial hardware interrupts
    extern "C" {
        fn SWI1_EGU1();
    // fn SWI2_EGU2();
    // fn SWI3_EGU3();
    }
};

#[exception]
unsafe fn DefaultHandler(_irqn: i16) -> ! {
    // On any unhandled faults, abort immediately
    // TODO: Probably want to log this or store it somewhere
    // so we can detect that a fault has happened?
    SCB::sys_reset()
}

fn try_send(
    uarte: &mut fleet_uarte::app::UarteApp<U1024, U1024>,
    msg: &Arbitrator,
) -> Result<(), ()> {
    match uarte.write_grant(128) {
        Ok(mut wgr) => {
            let used: usize = to_slice_cobs(&msg, &mut wgr)
                .map(|buf| buf.len())
                .unwrap_or(0);

            wgr.commit(used);
            Ok(())
        }
        Err(e) => {
            rprintln!("uartetxerr: {:?}", e);
            Err(())
        }
    }
}
