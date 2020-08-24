use anachro_client::Client;
use anachro_icd::{arbitrator::Arbitrator, ManagedString, PubSubPath};
use fleet_esb::{BorrowRxMessage, RollingTimer};
use {
    crate::timer::RollingRtcTimer,
    blinq::patterns,
    esb::consts::*,
    fleet_esb::{ptx::FleetRadioPtx, RxMessage},
    fleet_icd::radio::{DeviceToHost, GeneralDeviceMessage, HostToDevice},
    rtt_target::rprintln,
};

// Todo, this should probably be something trait-y
fn process_one(
    radio: &mut FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
    client: &mut Client,
    msg: Option<Arbitrator>,
) -> Result<(), ()> {
    let x = match client.process(&msg) {
        Ok(m) => m,
        Err(_e) => {
            rprintln!("sad client :(");
            return Err(());
        }
    };

    if let Some(bmsg) = x.broker_response {
        rprintln!("Sending {:?}", bmsg);
        radio.send(&bmsg, 0).unwrap();
    }

    if let Some(cmsg) = x.client_response {
        // TODO: I need some sort of way to handle things here,
        // probably deserializing to Owned data in some regard.
        // Return a Vec of plantlight messages?
        rprintln!("'{:?}': '{:?}'", cmsg.path, cmsg.payload);
    }

    Ok(())
}

fn processor(
    radio: &mut FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
    client: &mut Client,
) -> Result<(), ()> {
    let timer = RollingRtcTimer::new();

    let mut proc_flag = false;

    loop {
        match radio.receive_with() {
            Ok(mut msg) => {
                match msg.view_with(|msg: BorrowRxMessage<Arbitrator>| {
                    let smsg = Some(msg.msg);
                    process_one(radio, client, smsg).ok();
                }) {
                    Ok(()) => {
                        rprintln!("Successful send!");
                    }
                    Err(e) => {
                        rprintln!("Error send: {:?}", e);
                    }
                };

                // TODO
                msg.fgr.release();
                proc_flag = true;
            }
            Err(_) if !proc_flag => {
                let smsg = None;
                process_one(radio, client, smsg).ok();
                break;
            }
            Err(_) => break,
        }
    }

    Ok(())
}

pub enum CommsState {
    Connecting(u8),
    Subscribing {
        attempts: u8,
        paths_remaining: &'static [&'static str],
    },
    Steady,
}

static PATHS: &[&str] = &["plants/lights/living-room/+", "time/unix/local"];

pub fn rx_periodic(ctx: crate::rx_periodic::Context) {
    // Roughly 10ms
    const INTERVAL: i32 = crate::timer::SIGNED_TICKS_PER_SECOND / 100;
    // Roughly 100ms
    const POLL_PRX_INTERVAL: u32 = crate::timer::TICKS_PER_SECOND / 10;

    let esb_app = ctx.resources.esb_app;
    let client = ctx.resources.client;
    let comms_state = ctx.resources.comms_state;

    if let Err(_) = processor(esb_app, client) {
        client.reset_connection();
        *comms_state = CommsState::Connecting(0);
    }

    let next = match comms_state {
        CommsState::Connecting(ref n) => {
            if client.is_connected() {
                Some(CommsState::Subscribing {
                    attempts: 0,
                    paths_remaining: PATHS,
                })
            } else if *n >= 100 {
                client.reset_connection();
                Some(CommsState::Connecting(0))
            } else {
                Some(CommsState::Connecting(n + 1))
            }
        }
        CommsState::Subscribing {
            attempts,
            paths_remaining,
        } => {
            if !client.is_subscribe_pending() {
                if !paths_remaining.is_empty() {
                    let outgoing = client
                        .subscribe(PubSubPath::Long(ManagedString::Borrow(paths_remaining[0])))
                        .unwrap();
                    esb_app.send(&outgoing, 0).unwrap();

                    Some(CommsState::Subscribing {
                        attempts: 0,
                        paths_remaining: &paths_remaining[1..],
                    })
                } else {
                    Some(CommsState::Steady)
                }
            } else {
                if *attempts >= 100 {
                    client.reset_connection();
                    Some(CommsState::Connecting(0))
                } else {
                    Some(CommsState::Subscribing {
                        attempts: *attempts + 1,
                        paths_remaining,
                    })
                }
            }

        }
        CommsState::Steady => todo!(),
    };

    // 'rx: loop {
    //     let msg = esb_app.receive();

    //     // Got a message? Pet the dog.
    //     if let Ok(Some(_)) = &msg {
    //         ctx.resources.esb_wdog.pet();
    //         ctx.resources
    //             .blue_led
    //             .enqueue(patterns::blinks::LONG_ON_OFF);
    //     }

    //     match msg {
    //         Ok(None) => break 'rx,
    //         Ok(Some(RxMessage {
    //             msg: HostToDevice::PlantLight(m),
    //             ..
    //         })) => match ctx.spawn.relay_command(m) {
    //             Ok(_) => {}
    //             Err(e) => rprintln!("spawn err: {:?}", e),
    //         },
    //         Ok(Some(m)) => {
    //             rprintln!("Got unproc'd msg: {:?}", m);
    //         }
    //         Err(e) => {
    //             rprintln!("RxErr: {:?}", e);
    //         }
    //     }
    // }

    if esb_app.ticks_since_last_tx() > POLL_PRX_INTERVAL {
        match esb_app.send(&(), 0) {
            Ok(_) => { /*rprintln!("Sent {:?}", msg) */ }
            Err(e) => rprintln!("Send err: {:?}", e),
        }
    }

    ctx.schedule.rx_periodic(ctx.scheduled + INTERVAL).ok();
}
