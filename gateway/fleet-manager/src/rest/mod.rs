use std::thread::{JoinHandle, spawn};
use crate::Result;
use mvdb::Mvdb;
use fleet_icd::radio::{RelayState, RelayIdx};
use crate::plant::Plant;
use rocket::State;

#[get("/")]
fn index() -> &'static str {
    "Hello, world!"
}

#[post("/plant/<shelf>/force/<relay>/<setting>/<time_sec>")]
fn plant_override(shelf: usize, relay: usize, setting: String, time_sec: u64, plant: State<Plant>) -> String {
    if shelf != 0 {
        return format!("shelf {} is not a good shelf", shelf);
    }

    let stg = match setting.as_str() {
        "on" => true,
        "off" => false,
        other => return format!("What is '{}'?", other),
    };

    match plant.force(relay, stg, time_sec) {
        Ok(_) => {
            format!(
                "shelf: {}, relay: {}, forced {} for {} sec",
                shelf,
                relay,
                setting,
                time_sec,
            )
        }
        Err(e) => {
            format!("error: {:?}", e)
        }
    }
}

pub struct RestCtx {
    hdl: JoinHandle<()>,
}

impl RestCtx {
    pub fn new(plant: Plant) -> Self {
        RestCtx {
            hdl: spawn(move || {
                rocket::ignite()
                    .mount("/", routes![index, plant_override])
                    .manage(plant)
                    .launch();
            })
        }
    }

    pub fn join(self) -> Result<()> {
        self.hdl
            .join()
            .map(drop)
            .map_err(|_| String::from("oh...").into())
    }
}
