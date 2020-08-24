#![no_std]
#![no_main]

mod comms;
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

use anachro_client::Client;
use anachro_icd::Version;
use comms::CommsState;
use {
    blinq::{consts, patterns, Blinq},
    core::{default::Default, sync::atomic::AtomicBool},
    cortex_m::peripheral::SCB,
    cortex_m_rt::exception,
    esb::{
        consts::*, irq::StatePTX, Addresses, BBBuffer, ConfigBuilder, ConstBBBuffer, Error,
        EsbBuffer, EsbIrq, IrqTimer, TxPower,
    },
    fleet_esb::ptx::FleetRadioPtx,
    fleet_icd::radio::{DeviceToHost, PlantLightDeviceMessage, PlantLightHostMessage},
    fleet_keys::keys::KEY,
    hal::{
        clocks::LfOscConfiguration,
        gpio::{Level, Output, Pin, PushPull},
        pac::{RTC0, TIMER0},
        rtc::{RtcInterrupt, Started},
        wdt::{count, handles::HdlN, Parts as WatchdogParts, Watchdog, WatchdogHandle},
        Rng, Rtc,
    },
    panic_persist::get_panic_message_utf8,
    relays::Relays,
    rtt_target::{rprintln, rtt_init_print},
    timer::RollingRtcTimer,
};

#[rtic::app(device = crate::hal::pac, peripherals = true, monotonic = crate::timer::RollingRtcTimer)]
const APP: () = {
    struct Resources {
        esb_app: FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
        esb_irq: EsbIrq<U2048, U2048, TIMER0, StatePTX>,
        esb_timer: IrqTimer<TIMER0>,
        relays: Relays<RollingRtcTimer>,
        rtc: Rtc<RTC0, Started>,
        rtc_timer: RollingRtcTimer,
        relay_wdog: WatchdogHandle<HdlN>,
        esb_wdog: WatchdogHandle<HdlN>,

        blue_led: Blinq<consts::U8, Pin<Output<PushPull>>>,
        green_led: Blinq<consts::U8, Pin<Output<PushPull>>>,
        red_led: Blinq<consts::U8, Pin<Output<PushPull>>>,

        client: Client,
        comms_state: CommsState,
    }

    #[init(spawn = [relay_periodic, rx_periodic, relay_status, led_periodic])]
    fn init(ctx: init::Context) -> init::LateResources {
        // Set internal regulator voltage to 3v3 instead of 1v8
        if !ctx.device.UICR.regout0.read().vout().is_3v3() {
            // Enable erase
            ctx.device.NVMC.config.write(|w| w.wen().een());
            while ctx.device.NVMC.ready.read().ready().is_busy() {}

            // Erase regout0 page
            ctx.device.NVMC.erasepage().write(|w| unsafe {
                w.erasepage()
                    .bits(&ctx.device.UICR.regout0 as *const _ as u32)
            });
            while ctx.device.NVMC.ready.read().ready().is_busy() {}

            // enable write
            ctx.device.NVMC.config.write(|w| w.wen().wen());
            while ctx.device.NVMC.ready.read().ready().is_busy() {}

            // Set 3v3 setting
            ctx.device.UICR.regout0.write(|w| w.vout()._3v3());
            while ctx.device.NVMC.ready.read().ready().is_busy() {}

            // Return UCIR to read only
            ctx.device.NVMC.config.write(|w| w.wen().ren());
            while ctx.device.NVMC.ready.read().ready().is_busy() {}

            // system reset
            SCB::sys_reset();
        }

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
            .tx_power(TxPower::POS4DBM)
            .maximum_transmit_attempts(16)
            .check()
            .unwrap();
        let (esb_app, esb_irq, esb_timer) = BUFFER
            .try_split(ctx.device.TIMER0, ctx.device.RADIO, addresses, config)
            .unwrap();

        let esb_irq = esb_irq.into_ptx();

        // Create a new watchdog instance
        //
        // In case the watchdog is already running, just spin and let it expire, since
        // we can't configure it anyway. This usually happens when we first program
        // the device and the watchdog was previously active
        let (relay_wdog, esb_wdog) = match Watchdog::try_new(ctx.device.WDT) {
            Ok(mut watchdog) => {
                // Set the watchdog to timeout after 5 minutes (in 32.768kHz ticks)
                watchdog.set_lfosc_ticks(5 * 60 * 32768);

                watchdog.run_during_debug_halt(true);
                watchdog.run_during_sleep(true);

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

        let (relay_wdog, esb_wdog) = (relay_wdog.degrade(), esb_wdog.degrade());

        rtt_init_print!();

        if let Some(msg) = get_panic_message_utf8() {
            // write the panic message
            rprintln!("{}", msg);
        } else {
            rprintln!("Starting clean!");
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
        ctx.spawn.led_periodic().ok();

        let mut blue = Blinq::new(p0.p0_12.into_push_pull_output(Level::High).degrade(), true);
        let mut red = Blinq::new(p0.p0_08.into_push_pull_output(Level::High).degrade(), true);
        let mut green = Blinq::new(p1.p1_09.into_push_pull_output(Level::High).degrade(), true);

        // Insert 3s of all white short blink on reset
        for _ in 0..3 {
            red.enqueue(patterns::blinks::QUARTER_DUTY);
            green.enqueue(patterns::blinks::QUARTER_DUTY);
            blue.enqueue(patterns::blinks::QUARTER_DUTY);
        }

        let client = Client::new(
            "plant light - living room",
            Version {
                major: 0,
                minor: 0,
                trivial: 1,
                misc: 222,
            },
            rng.random_u16(),
        );

        init::LateResources {
            esb_app: radio,
            esb_irq,
            esb_timer,
            relays,
            rtc,
            relay_wdog,
            esb_wdog,
            rtc_timer: RollingRtcTimer::new(),
            blue_led: blue,
            red_led: red,
            green_led: green,
            client,
            comms_state: CommsState::Connecting(0),
        }
    }

    /// We don't do anything in idle. Just loop waiting for events
    #[idle(resources = [green_led])]
    fn idle(mut ctx: idle::Context) -> ! {
        loop {
            ctx.resources.green_led.lock(|l| {
                if l.idle() {
                    l.enqueue(patterns::blinks::LONG_ON_OFF);
                }
            });
            cortex_m::asm::wfi();
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
            Ok(_state) => {}
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
    #[task(schedule = [relay_status], resources = [relays, esb_app, relay_wdog, red_led])]
    fn relay_status(ctx: relay_status::Context) {
        const INTERVAL: i32 = timer::SIGNED_TICKS_PER_SECOND * 3;

        // Check the relays? Pet the dog.
        ctx.resources.relay_wdog.pet();
        ctx.resources
            .red_led
            .enqueue(patterns::blinks::MEDIUM_ON_OFF);

        let stat = ctx.resources.relays.current_state();
        let msg = DeviceToHost::PlantLight(PlantLightDeviceMessage::Status(stat));
        ctx.resources.esb_app.send(&msg, 0).ok();

        ctx.schedule.relay_status(ctx.scheduled + INTERVAL).ok();
    }

    #[task(schedule = [led_periodic], resources = [red_led, blue_led, green_led])]
    fn led_periodic(ctx: led_periodic::Context) {
        ctx.resources.red_led.step();
        ctx.resources.green_led.step();
        ctx.resources.blue_led.step();

        ctx.schedule
            .led_periodic(ctx.scheduled + (timer::SIGNED_TICKS_PER_SECOND / 4))
            .ok();
    }

    /// This software event fires periodically, processing any incoming messages
    ///
    /// We also periodically poll the remote device to check if any messages
    /// are pending.
    ///
    /// We also also check to see if we haven't heard from the remote device in
    /// a while. If so, we reboot.
    #[task(schedule = [rx_periodic], spawn = [relay_command], resources = [esb_app, esb_wdog, blue_led, client, comms_state])]
    fn rx_periodic(ctx: rx_periodic::Context) {
        comms::rx_periodic(ctx);
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

#[exception]
unsafe fn DefaultHandler(irqn: i16) -> ! {
    rprintln!("uh oh! {}", irqn);
    // On any unhandled faults, abort immediately
    // TODO: Probably want to log this or store it somewhere
    // so we can detect that a fault has happened?
    SCB::sys_reset()
}
