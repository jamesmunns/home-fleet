use anachro_client::{Client, ClientError, ClientIo, RecvMsg};
use anachro_icd::{arbitrator::Arbitrator, component::Component, ManagedString, PubSubPath};
use fleet_esb::{BorrowRxMessage, RollingTimer};
use {
    crate::timer::RollingRtcTimer,
    blinq::patterns,
    esb::consts::*,
    fleet_esb::{ptx::FleetRadioPtx, RxMessage},
    fleet_icd::radio::{DeviceToHost, GeneralDeviceMessage, HostToDevice},
    fleet_icd::radio2::PlantLightTable,
    postcard::to_slice,
    rtt_target::rprintln,
};

use heapless::{consts, ArrayLength, Vec};

use anachro_client::from_bytes;
use fleet_esb::ptx::PayloadR;

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
                        return Ok(None);
                    }
                } else {
                    // What?
                    return Ok(None);
                }
            }
            Err(_) => return Ok(None),
        }
    }
    fn send(&mut self, msg: &Component) -> Result<(), ClientError> {
        self.esb_app
            .send(msg, 0)
            .map_err(|_| ClientError::OutputFull)
    }
}

impl<'a> IoHandler<'a> {
    pub fn drop_grant(&mut self) {
        let _ = self.rgr.take();
    }
}

pub fn publish(ctx: crate::publish::Context, msg: &PlantLightTable) {
    let esb_app = ctx.resources.esb_app;
    let client = ctx.resources.client;

    let mut io = IoHandler { esb_app, rgr: None };

    let mut buf = [0u8; 128];

    //  TODO - can I do this automatically?
    let pubby = match msg.serialize(&mut buf) {
        Ok(pb) => pb,
        Err(_) => return,
    };

    match client.publish(&mut io, pubby.path, pubby.buf) {
        Ok(_) => rprintln!("Sent Pub!"),
        Err(_) => rprintln!("Pub Send Error!"),
    }
}

pub fn rx_periodic(ctx: crate::rx_periodic::Context) {
    // Roughly 10ms
    const INTERVAL: i32 = crate::timer::SIGNED_TICKS_PER_SECOND / 100;
    // Roughly 100ms
    const POLL_PRX_INTERVAL: u32 = crate::timer::TICKS_PER_SECOND / 10;

    let esb_app = ctx.resources.esb_app;
    let client = ctx.resources.client;

    let mut io = IoHandler { esb_app, rgr: None };

    match client.process_one::<_, PlantLightTable>(&mut io) {
        Ok(Some(RecvMsg {
            payload: PlantLightTable::Relay(cmd),
            ..
        })) => {
            rprintln!("Set relay!");
            ctx.spawn.relay_command(cmd).ok();
        }
        Ok(Some(msg)) => {
            rprintln!("GOT {:?}", msg);
        }
        Ok(None) => {}
        Err(e) => {
            rprintln!("ERR: {:?}", e);
        }
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
