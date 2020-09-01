use serde::{Serialize, Deserialize};

use anachro_client::pubsub_table;

pub fn matches(subscr: &str, publ: &str) -> bool {
    if subscr.is_empty() || publ.is_empty() {
        return false;
    }

    let mut s_iter = subscr.split("/");
    let mut p_iter = publ.split("/");

    loop {
        match (s_iter.next(), p_iter.next()) {
            (Some("+"), Some(_)) => continue,
            (Some("#"), _) | (None, None) => return true,
            (Some(lhs), Some(rhs)) if lhs == rhs => continue,
            _ => return false,
        }
    }
}

use crate::radio::{
    RelayIdx,
    RelayState,
    ShelfStatus,
};

#[derive(Serialize, Deserialize, Debug)]
pub struct RelayCommand {
    pub relay: RelayIdx,
    pub state: RelayState,
}

pubsub_table!(
    PlantLightTable,
    // ====================
    Subs => {
        Relay: "lights/plants/living-room/set"  => RelayCommand,
        Time:  "time/unix/local"                => u32,
    },
    Pubs => {
        Status: "lights/plants/living-room/status" => ShelfStatus,
    },
);
