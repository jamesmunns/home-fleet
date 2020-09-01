#![feature(proc_macro_hygiene, decl_macro)]

use mvdb::Mvdb;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{sleep, spawn};
use std::time::Duration;

#[macro_use]
extern crate rocket;

use anachro_client::pubsub_table;

use fleet_icd::{
    radio2::RelayCommand,
    radio::ShelfStatus,
};

pubsub_table!(
    HomeFleetTable,
    // ====================
    Subs => {
        Status: "lights/plants/living-room/status" => ShelfStatus,
    },
    Pubs => {
        Relay: "lights/plants/living-room/set"  => RelayCommand,
        Time:  "time/unix/local"                => u32,
    },
);

mod comms;
mod plant;
mod rest;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Options {
    plants: [PlantOptions; 1],
    uart: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PlantOptions {
    pipe: u8,
    stat_file: PathBuf,
    opt_file: PathBuf,
}

pub struct Channels {
    pub tx: Sender<HomeFleetTable>,
    pub rx: Receiver<HomeFleetTable>,
}

pub struct Route {
    pub paths: &'static [&'static str],
    pub comms: Channels,
}

pub struct Comms {
    pub router: Route,
    pub task: Channels,
}

impl Comms {
    fn new(paths: &'static [&'static str]) -> Self {
        let (txa, rxa) = channel();
        let (txb, rxb) = channel();
        Comms {
            router: Route {
                paths,
                comms: Channels {
                    tx: txa,
                    rx: rxb,
                }
            },
            task: Channels {
                tx: txb,
                rx: rxa,
            }
        }
    }
}


pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

fn main() -> Result<()> {
    let main_cfg_path = PathBuf::from("main_cfg.mvdb.json");
    let main_cfg: Mvdb<Options> = Mvdb::from_file_pretty(&main_cfg_path)?;

    let plants = main_cfg.access(|t| t.plants.clone())?;
    let uart = main_cfg.access(|t| t.uart.clone())?;

    let Comms {
        router: router_plants,
        task: task_plants,
    } = Comms::new(&["lights/plants/living-room/status"]);

    let mut modem = comms::CommsCtx::new(&uart, vec![router_plants])?;

    // TODO: Loop
    let plant = plant::Plant::new(&plants[0].opt_file, &plants[0].stat_file, task_plants)?;
    let plant2 = plant.clone();

    let plant_hdl = spawn(move || {
        loop {
            match plant.poll() {
                Ok(_) => {
                    sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    println!("boop plant: {:?}", e);
                    std::process::exit(1);
                }
            }
        }
    });

    let modem_hdl = spawn(move || {
        loop {
            match modem.poll() {
                Ok(_) => {
                    sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    println!("boop modem: {:?}", e);
                    std::process::exit(1);
                }
            }
        }
    });

    let rest_hdl = rest::RestCtx::new(plant2);

    plant_hdl.join().unwrap();
    modem_hdl.join().unwrap();
    rest_hdl.join().unwrap();

    println!("Hello, world!");
    Ok(())
}
