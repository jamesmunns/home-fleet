use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum HostToDevice {
    General(GeneralHostMessage),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum DeviceToHost {
    General(GeneralDeviceMessage),
    PlantLight(PlantLightDeviceMessage),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum GeneralHostMessage {
    Ping,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum GeneralDeviceMessage {
    Pong,
    InitializeSession,
    MessageRequest,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum PlantLightDeviceMessage {
    Status(ShelfStatus),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum PlantLightHostMessage {
    SetRelay {
        relay: u8,
        state: bool,
    },
    SetCounters {
        on_lifetime: u32,
        off_lifetime: u32,
    }
}


#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct ShelfStatus {
    relays: [RelayStatus; 4]
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct RelayStatus {
    enabled: bool,
    seconds_in_state: u32,
    seconds_on_lifetime: u32,
    seconds_off_lifetime: u32,
}


