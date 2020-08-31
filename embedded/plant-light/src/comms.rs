use anachro_client::{Client, ClientIo, ClientError};
use anachro_icd::{arbitrator::Arbitrator, component::Component, ManagedString, PubSubPath};
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

use heapless::{ArrayLength, Vec, consts};

// fn processor<N>(
//     radio: &mut FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
//     client: &mut Client,
// ) -> Result<Vec<PlantLightTable, N>, ()>
// where
//     N: ArrayLength<PlantLightTable>,
// {
//     let timer = RollingRtcTimer::new();

//     let mut proc_flag = false;

//     loop {
//         match radio.receive_with() {
//             Ok(mut msg) => {
//                 msg.view_with(|msg: BorrowRxMessage<Arbitrator>| {
//                     let smsg = Some(msg.msg);
//                     process_one(radio, client, smsg).ok();
//                 }).map_err(drop)?;

//                 proc_flag = true;
//             }
//             Err(_) if !proc_flag => {
//                 let smsg = None;
//                 process_one(radio, client, smsg).ok();
//                 break;
//             }
//             Err(_) => break,
//         }
//     }

//     Ok(Vec::new())
// }

use fleet_esb::ptx::PayloadR;
use anachro_client::from_bytes;

struct IoHandler<'a> {
    esb_app: &'a mut FleetRadioPtx<U2048, U2048, RollingRtcTimer>,
    rgr: Option<PayloadR<U2048>>,
}

impl<'a> ClientIo for IoHandler<'a> {
    fn recv(&mut self) -> Result<Option<Arbitrator>, ClientError> {
        self.drop_grant();

        match self.esb_app.just_gimme_frame() {
            Ok(msg) => {
                self.rgr = Some(msg);
                if let Some(ref msg) = self.rgr {
                    if let Ok(msg) = from_bytes::<Arbitrator>(msg) {
                        return Ok(Some(msg));
                    } else {
                        return Ok(None)
                    }
                } else {
                    // What?
                    return Ok(None)
                }
            }
            Err(_) => {
                return Ok(None)
            }
        }
    }
    fn send(&mut self, msg: &Component) -> Result<(), ClientError> {
        self.esb_app.send(msg, 0).map_err(|_| ClientError::OutputFull)
    }
}

impl<'a> IoHandler<'a> {
    pub fn drop_grant(&mut self) {
        let _ = self.rgr.take();
    }
}

pub fn rx_periodic(ctx: crate::rx_periodic::Context) {
    // Roughly 10ms
    const INTERVAL: i32 = crate::timer::SIGNED_TICKS_PER_SECOND / 100;
    // Roughly 100ms
    const POLL_PRX_INTERVAL: u32 = crate::timer::TICKS_PER_SECOND / 10;

    let esb_app = ctx.resources.esb_app;
    let client = ctx.resources.client;

    let mut io = IoHandler {
        esb_app,
        rgr: None,
    };

    match client.process_one::<_, PlantLightTable>(&mut io) {
        Ok(Some(msg)) => {
            rprintln!("GOT {:?}", msg);
        },
        Ok(None) => {},
        Err(e) => {
            rprintln!("ERR: {:?}", e);
        },
    }

    io.drop_grant();

    if esb_app.ticks_since_last_tx() > POLL_PRX_INTERVAL {
        match esb_app.send(&(), 0) {
            Ok(_) => { /*rprintln!("Sent {:?}", msg) */ }
            Err(e) => rprintln!("Send err: {:?}", e),
        }
    }

    ctx.schedule.rx_periodic(ctx.scheduled + INTERVAL).ok();
}

