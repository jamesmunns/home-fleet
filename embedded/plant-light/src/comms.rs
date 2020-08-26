use anachro_client::Client;
use anachro_icd::{arbitrator::Arbitrator, ManagedString, PubSubPath};
use fleet_esb::{BorrowRxMessage, RollingTimer};
use {
    crate::timer::RollingRtcTimer,
    blinq::patterns,
    esb::consts::*,
    fleet_esb::{ptx::FleetRadioPtx, RxMessage},
    fleet_icd::radio::{DeviceToHost, GeneralDeviceMessage, HostToDevice},
    fleet_icd::radio2::PlantLightTable,
    rtt_target::rprintln,
};

// Todo, this should probably be something trait-y
fn process_one(
    radio: &mut FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
    client: &mut Client,
    msg: Option<Arbitrator>,
) -> Result<Option<PlantLightTable>, ()> {
    let x = match client.process(&msg) {
        Ok(m) => m,
        Err(_e) => {
            rprintln!("sad client :(");
            return Err(());
        }
    };

    if let Some(bmsg) = x.broker_response {
        rprintln!("Sending {:?}", bmsg);
        radio.send(&bmsg, 0).map_err(drop)?;
    }

    if let Some(cmsg) = x.client_response {
        Ok(PlantLightTable::from_pub_sub(cmsg).ok())
    } else {
        Ok(None)
    }
}

use heapless::{ArrayLength, Vec, consts};

fn processor<N>(
    radio: &mut FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
    client: &mut Client,
) -> Result<Vec<PlantLightTable, N>, ()>
where
    N: ArrayLength<PlantLightTable>,
{
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

    Ok(Vec::new())
}

pub enum CommsState {
    Connecting(u8),
    Subscribing {
        attempts: u8,
        paths_remaining: &'static [&'static str],
    },
    Steady,
}

pub fn rx_periodic(ctx: crate::rx_periodic::Context) {
    // Roughly 10ms
    const INTERVAL: i32 = crate::timer::SIGNED_TICKS_PER_SECOND / 100;
    // Roughly 100ms
    const POLL_PRX_INTERVAL: u32 = crate::timer::TICKS_PER_SECOND / 10;

    let esb_app = ctx.resources.esb_app;
    let client = ctx.resources.client;
    let comms_state = ctx.resources.comms_state;

    match processor::<consts::U16>(esb_app, client) {
        Ok(msgs) => {
            for msg in msgs {
                match msg {
                    PlantLightTable::Relay(cmd) => {
                        rprintln!("RELAY CMD: {:?}", cmd);
                    }
                    PlantLightTable::Time(time) => {
                        rprintln!("THE TIME IS {}", time);
                    }
                }
            }
        }
        Err(e) => {
            rprintln!("Error: {:?}, resetting connection", e);
            client.reset_connection();
            *comms_state = CommsState::Connecting(0);
        }
    }

    let next = match comms_state {
        CommsState::Connecting(ref n) => {
            // We are waiting for a connection
            if client.is_connected() {
                // We are connected! Start subscribing
                Some(CommsState::Subscribing {
                    attempts: 0,
                    paths_remaining: PlantLightTable::paths(),
                })
            } else if *n >= 100 {
                // We've waited too long. Try again
                client.reset_connection();
                Some(CommsState::Connecting(0))
            } else {
                // Keep waiting for that connection
                Some(CommsState::Connecting(n + 1))
            }
        }
        CommsState::Subscribing {
            attempts,
            paths_remaining,
        } => {
            // Are we waiting for a subscription?
            if !client.is_subscribe_pending() {
                // No, are there any pending subscriptions?
                if !paths_remaining.is_empty() {
                    // Yup! 'pop' the first item off the list, subscribe, and start waiting
                    let outgoing = client
                        .subscribe(PubSubPath::Long(ManagedString::Borrow(paths_remaining[0])))
                        .unwrap();
                    esb_app.send(&outgoing, 0).unwrap();

                    Some(CommsState::Subscribing {
                        attempts: 0,
                        paths_remaining: &paths_remaining[1..],
                    })
                } else {
                    // No pending, no remaining, we're good to go!
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
        CommsState::Steady => None,
    };

    if let Some(state) = next {
        *comms_state = state;
    }

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

