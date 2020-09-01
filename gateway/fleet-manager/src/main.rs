#![feature(proc_macro_hygiene, decl_macro)]

use fleet_icd::radio::{DeviceToHost, HostToDevice};
use mvdb::Mvdb;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{sleep, spawn};
use std::time::Duration;

#[macro_use]
extern crate rocket;

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

pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

fn main() -> Result<()> {
    let main_cfg_path = PathBuf::from("main_cfg.mvdb.json");
    let main_cfg: Mvdb<Options> = Mvdb::from_file_pretty(&main_cfg_path)?;

    let plants = main_cfg.access(|t| t.plants.clone())?;
    let uart = main_cfg.access(|t| t.uart.clone())?;

    let (plant_app, plant_mdm) = comms(plants[0].pipe);

    let mut modem = comms::CommsCtx::new(&uart, vec![plant_mdm])?;

    // TODO: Loop
    let plant = plant::Plant::new(&plants[0].opt_file, &plants[0].stat_file, plant_app)?;
    let plant2 = plant.clone();

    let plant_hdl = spawn(move || {
        while plant.poll().is_ok() {
            sleep(Duration::from_millis(250));
        }
        std::process::exit(1);
    });

    let modem_hdl = spawn(move || {
        while modem.poll().is_ok() {
            sleep(Duration::from_millis(50));
        }
        std::process::exit(1);
    });

    let rest_hdl = rest::RestCtx::new(plant2);

    plant_hdl.join().unwrap();
    modem_hdl.join().unwrap();
    rest_hdl.join().unwrap();

    println!("Hello, world!");
    Ok(())
}

fn comms(pipe: u8) -> (AppCommsHandle, ModemCommsHandle) {
    let (out_tx, out_rx) = channel();
    let (in_tx, in_rx) = channel();

    (
        AppCommsHandle {
            tx: out_tx,
            rx: in_rx,
            pipe,
        },
        ModemCommsHandle {
            tx: in_tx,
            rx: out_rx,
            pipe,
        },
    )
}

pub struct AppCommsHandle {
    tx: Sender<HostToDevice>,
    rx: Receiver<DeviceToHost>,
    pipe: u8,
}

pub struct ModemCommsHandle {
    tx: Sender<DeviceToHost>,
    rx: Receiver<HostToDevice>,
    pipe: u8,
}
