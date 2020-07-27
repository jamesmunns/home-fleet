use crate::radio::{HostToDevice, DeviceToHost};
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum PcToModem {
    Outgoing {
        pipe: u8,
        msg: HostToDevice,
    },
    Ping,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum ModemToPc {
    Incoming {
        pipe: u8,
        msg: DeviceToHost,
    },
    Pong,
}
