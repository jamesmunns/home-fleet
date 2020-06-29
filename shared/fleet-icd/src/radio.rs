use core::convert::TryFrom;
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

impl From<bool> for RelayState {
    fn from(other: bool) -> Self {
        match other {
            true => RelayState::On,
            false => RelayState::Off,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum RelayIdx {
    Relay0,
    Relay1,
    Relay2,
    Relay3,
}

impl Into<usize> for RelayIdx {
    fn into(self) -> usize {
        match self {
            RelayIdx::Relay0 => 0,
            RelayIdx::Relay1 => 1,
            RelayIdx::Relay2 => 2,
            RelayIdx::Relay3 => 3,
        }
    }
}

impl TryFrom<usize> for RelayIdx {
    // fn try_from(other: &usize) -> Result<
    type Error = ();

    fn try_from(other: usize) -> core::result::Result<Self, Self::Error> {
        match other {
            0 => Ok(RelayIdx::Relay0),
            1 => Ok(RelayIdx::Relay1),
            2 => Ok(RelayIdx::Relay2),
            3 => Ok(RelayIdx::Relay3),
            _ => Err(())
        }
    }
}


#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct ShelfStatus {
    pub relays: [RelayStatus; 4]
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct RelayStatus {
    pub enabled: RelayState,
    pub seconds_in_state: u32,
}


