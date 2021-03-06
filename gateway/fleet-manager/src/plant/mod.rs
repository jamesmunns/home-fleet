use crate::{Result, Channels, HomeFleetTable};
use chrono::{
    naive::{NaiveDate, NaiveTime},
    Local,
};
use fleet_icd::radio::{
    RelayState as RadioRelayState, ShelfStatus
};
use fleet_icd::radio2::RelayCommand;
use mvdb::Mvdb;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::TryInto;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use topq::{consts::*, Timer, Topq};

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
    start: Instant,
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
    comms: Channels,
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
    pub fn new(opts: &Path, stats: &Path, comms: Channels) -> Result<Self> {
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
                ],
            })),
        })
    }

    pub fn force(&self, idx: usize, setting: bool, duration_sec: u64) -> Result<()> {
        let mut state = self.inner.lock().map_err(|_| String::from("ohhh!1111!!"))?;
        state
            .state
            .get_mut(idx)
            .ok_or_else(|| String::from("errrr"))?
            .insert(setting, RelayPriority::Override, duration_sec);
        Ok(())
    }

    pub fn poll(&self) -> Result<()> {
        let options_copy = self.options.access(|t| t.clone())?;

        // Should we be on right now?
        let now = Local::now().time();
        let be_on = (now >= options_copy.start_time) && (now <= options_copy.end_time);

        let mut result = vec![];
        {
            let mut state = self.inner.lock().map_err(|_t| "lol".to_string())?;
            for relay in state.state.iter_mut() {
                relay.insert(be_on, RelayPriority::Scheduled, 15);
                result.push(*relay.get_data().ok_or_else(|| "wtf".to_string())?);
            }

            let should_tx = match state.last_tx {
                None => true,

                // TODO: This should be an option
                Some(inst) if inst.elapsed() >= Duration::from_secs(3) => true,
                Some(_) => false,
            };

            if should_tx {
                state.last_tx = Some(Instant::now());
                for (idx, rstate) in result.drain(..).enumerate() {
                    state.comms.tx.send(
                        HomeFleetTable::Relay(RelayCommand {
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
                    HomeFleetTable::Status(ShelfStatus { relays }) => {
                        for (idx, relay) in relays.iter().enumerate() {
                            if relay.enabled == RadioRelayState::On {
                                // TODO: Update state
                                println!("relay {} is on", idx);
                            }
                        }
                    }
                    other => {
                        println!("other: {:?}", other);
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
