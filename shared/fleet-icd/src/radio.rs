use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum HostToDevice {
    General(GeneralHostMessage),
    PlantLight(PlantLightHostMessage),
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
        relay: RelayIdx,
        state: RelayState,
    },
    SetCounters {
        on_lifetime: u32,
        off_lifetime: u32,
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum RelayState {
    Off,
    On,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum RelayIdx {
    Relay0,
    Relay1,
    Relay2,
    Relay3,
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


