#![no_std]
#![no_main]

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
        sync::atomic::AtomicBool,
    },
    esb::{
        consts::*, irq::StatePTX, Addresses, BBBuffer, ConfigBuilder, ConstBBBuffer, Error,
        EsbBuffer, EsbIrq, IrqTimer,
    },
    fleet_esb::{ptx::FleetRadioPtx, RxMessage},
    fleet_icd::radio::{
        DeviceToHost, GeneralDeviceMessage, HostToDevice, PlantLightDeviceMessage,
        PlantLightHostMessage,
    },
    fleet_keys::keys::KEY,
    hal::{
        clocks::LfOscConfiguration,
        pac::{RTC0, TIMER0},
        rtc::{RtcInterrupt, Started},
        Rng, Rtc,
    },
    panic_persist::get_panic_message_utf8,
    relays::Relays,
    rtt_target::{rprintln, rtt_init_print},
    timer::RollingRtcTimer,
};

#[rtfm::app(device = crate::hal::pac, peripherals = true, monotonic = crate::timer::RollingRtcTimer)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPtx<U2048, U2048, DeviceToHost, HostToDevice, RollingRtcTimer>,
        esb_irq: EsbIrq<U2048, U2048, TIMER0, StatePTX>,
        esb_timer: IrqTimer<TIMER0>,
        relays: Relays<RollingRtcTimer>,
        rtc: Rtc<RTC0, Started>,
        rtc_timer: RollingRtcTimer,
    }

    #[init(spawn = [relay_periodic, rx_periodic, relay_status])]
    fn init(ctx: init::Context) -> init::LateResources {
        let clocks = hal::clocks::Clocks::new(ctx.device.CLOCK);
        let clocks = clocks.enable_ext_hfosc();
        let clocks = clocks.set_lfclk_src_external(LfOscConfiguration::NoExternalNoBypass);
        clocks.start_lfclk();

        let p0 = hal::gpio::p0::Parts::new(ctx.device.P0);
        let p1 = hal::gpio::p1::Parts::new(ctx.device.P1);

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

        if let Some(msg) = get_panic_message_utf8() {
            // write the panic message
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

        let relays = Relays::from_pins(
            [
                p1.p1_10.degrade(),
                p1.p1_13.degrade(),
                p1.p1_15.degrade(),
                p0.p0_02.degrade(),
            ],
            RollingRtcTimer::new(),
        );

        // Spawn the periodic tasks so they can self-reschedule
        ctx.spawn.rx_periodic().ok();
        ctx.spawn.relay_periodic().ok();
        ctx.spawn.relay_status().ok();

        init::LateResources {
            esb_app: radio,
            esb_irq,
            esb_timer,
            relays,
            rtc,
            rtc_timer: RollingRtcTimer::new(),
        }
    }


    /// We don't do anything in idle. Just loop waiting for events
    #[idle]
    fn idle(_ctx: idle::Context) -> ! {
        loop {
            cortex_m::asm::wfe();
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

    /// This event fires every time there is a radio event
    ///
    /// This is largely to drive the ESB protocol driver
    #[task(binds = RADIO, resources = [esb_irq], priority = 3)]
    fn radio_interrupt(ctx: radio_interrupt::Context) {
        match ctx.resources.esb_irq.radio_interrupt() {
            Err(Error::MaximumAttempts) => rprintln!("max attempts!"),
            Err(e) => panic!("Found error {:?}", e),
            Ok(_state) => {},
        }
    }

    /// This event fires when the timer interrupt goes off
    ///
    /// This is to drive timeouts on the ESB protocol driver
    #[task(binds = TIMER0, resources = [esb_timer], priority = 3)]
    fn timer0(ctx: timer0::Context) {
        ctx.resources.esb_timer.timer_interrupt();
    }

    /// This software event fires periodically to check for timeouts
    /// of the relays
    #[task(schedule = [relay_periodic], resources = [relays])]
    fn relay_periodic(ctx: relay_periodic::Context) {
        ctx.resources.relays.check_timeout();
        ctx.schedule
            .relay_periodic(ctx.scheduled + timer::SIGNED_TICKS_PER_SECOND)
            .ok();
    }

    /// This software event fires periodically, sending the current status
    /// of the relays over the radio
    #[task(schedule = [relay_status], resources = [relays, esb_app])]
    fn relay_status(ctx: relay_status::Context) {
        const INTERVAL: i32 = timer::SIGNED_TICKS_PER_SECOND * 3;

        let stat = ctx.resources.relays.current_state();
        let msg = DeviceToHost::PlantLight(PlantLightDeviceMessage::Status(stat));
        ctx.resources.esb_app.send(&msg, 0).ok();

        ctx.schedule.relay_status(ctx.scheduled + INTERVAL).ok();
    }

    /// This software event fires periodically, processing any incoming messages
    ///
    /// We also periodically poll the remote device to check if any messages
    /// are pending.
    ///
    /// We also also check to see if we haven't heard from the remote device in
    /// a while. If so, we reboot.
    #[task(schedule = [rx_periodic], spawn = [relay_command], resources = [esb_app])]
    fn rx_periodic(ctx: rx_periodic::Context) {
        // Roughly 10ms
        const INTERVAL: i32 = timer::SIGNED_TICKS_PER_SECOND / 100;
        // Roughly 100ms
        const POLL_PRX_INTERVAL: u32 = timer::TICKS_PER_SECOND / 10;
        // Roughly 60s
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

    /// This software event is triggered whenever a relay message arrives
    #[task(resources = [relays], capacity = 5)]
    fn relay_command(ctx: relay_command::Context, cmd: PlantLightHostMessage) {
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
