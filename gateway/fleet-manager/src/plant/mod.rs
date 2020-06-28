use serde::{Serialize, Deserialize};
use mvdb::Mvdb;
use crate::{AppCommsHandle, Result};
use std::path::Path;
use chrono::{
    Local,
    naive::{NaiveTime, NaiveDate},
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, Duration};
use topq::{Topq, consts::*, Timer};
use std::convert::TryInto;
use fleet_icd::radio::{HostToDevice, DeviceToHost, PlantLightHostMessage, PlantLightDeviceMessage, RelayState as RadioRelayState};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShelfOptions {
    hours_per_day: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlantOptions {
    shelf_opts: [ShelfOptions; 4],
    start_time: NaiveTime,
    end_time: NaiveTime,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DayStat {
    minutes_on: f64,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct PlantStats {
    date_map: HashMap<NaiveDate, [DayStat; 4]>,
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RelayPriority {
    Scheduled,
    Override,
}

#[derive(Clone)]
struct InstantTimer {
    start: Instant
}

impl Default for InstantTimer {
    fn default() -> Self {
        InstantTimer {
            start: Instant::now(),
        }
    }
}

impl Timer for InstantTimer {
    const TICKS_PER_SECOND: u32 = 1;
    type Time = u64;

    fn wrapping_add(a: &Self::Time, b: &Self::Time) -> Self::Time {
        a.wrapping_add(*b)
    }

    fn now(&self) -> Self::Time {
        self.start.elapsed().as_secs()
    }
}

type RelayState = Topq<bool, RelayPriority, InstantTimer, U4>;

pub struct InnerState {
    comms: AppCommsHandle,
    last_rx: Option<Instant>,
    last_tx: Option<Instant>,
    state: [RelayState; 4],
}

#[derive(Clone)]
pub struct Plant {
    options: Mvdb<PlantOptions>,
    stats: Mvdb<PlantStats>,
    inner: Arc<Mutex<InnerState>>,
}

impl Plant {
    pub fn new(opts: &Path, stats: &Path, comms: AppCommsHandle) -> Result<Self> {
        let timer = InstantTimer::default();

        Ok(Self {
            options: Mvdb::from_file_pretty(opts)?,
            stats: Mvdb::from_file_or_default_pretty(stats)?,
            inner: Arc::new(Mutex::new(InnerState {
                comms,
                last_rx: None,
                last_tx: None,
                state: [
                    Topq::new(timer.clone()),
                    Topq::new(timer.clone()),
                    Topq::new(timer.clone()),
                    Topq::new(timer.clone()),
                ]
            })),
        })
    }

    pub fn poll(&self) -> Result<()> {
        let options_copy = self.options.access(|t| t.clone())?;

        // Should we be on right now?
        let now = Local::now().time();
        let be_on = (now >= options_copy.start_time) && (now <= options_copy.end_time);

        let mut result = vec![];
        {
            let mut state = self.inner.lock().map_err(|t| "lol".to_string())?;
            for relay in state.state.iter_mut() {
                relay.insert(be_on, RelayPriority::Scheduled, 15);
                result.push(*relay.get_data().unwrap());
            }

            let should_tx = match state.last_tx {
                None => true,

                // TODO: This should be an option
                Some(inst) if inst.elapsed() >= Duration::from_secs(3) => {
                    true
                }
                Some(_) => false,
            };

            if should_tx {
                state.last_tx = Some(Instant::now());

                for (idx, rstate) in result.drain(..).enumerate() {
                    state.comms.tx.send(
                        HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                            relay: idx.try_into().map_err(|_| String::from("whoops"))?,
                            state: rstate.into(),
                        })
                    )?;
                }
            }

            let mut has_rx = false;
            while let Ok(msg) = state.comms.rx.try_recv() {
                has_rx = true;
                match msg {
                    DeviceToHost::PlantLight(pl) => {
                        match pl {
                            PlantLightDeviceMessage::Status(pstat) => {
                                for (idx, relay) in pstat.relays.iter().enumerate() {
                                    if relay.enabled == RadioRelayState::On {
                                        // TODO: Update state
                                        println!("relay {} is on", idx);
                                    }
                                }
                            }
                        }
                    }
                    DeviceToHost::General(gen) => {
                        println!("general: {:?}", gen);
                    }
                }
            }

            if has_rx {
                state.last_rx = Some(Instant::now());
            }
        }

        Ok(())
    }
}

