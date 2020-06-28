use chrono::prelude::*;
use chrono::{DateTime, Local, TimeZone};
use postcard::{from_bytes, to_slice_cobs};
use serde::{Deserialize, Serialize};
use serialport::prelude::*;
use std::{
    collections::{HashMap, VecDeque},
    fs::{read_to_string, File, OpenOptions},
    io::{self, prelude::*},
    path::Path,
    sync::mpsc::{Receiver, Sender, TryRecvError},
    thread::sleep,
    time::{Duration, Instant},
};
use fleet_icd::{
    consts::*, Buffer, FeedResult,
    radio::{DeviceToHost, HostToDevice, GeneralHostMessage, PlantLightHostMessage, RelayIdx, RelayState},
    modem::{PcToModem, ModemToPc},
};
use crate::{ModemCommsHandle, Result};

pub struct CommsCtx {
    port: Box<dyn SerialPort>,
    map: HashMap<u8, ModemCommsHandle>,
    cobs_buf: Buffer<U256>,
}

impl CommsCtx {
    pub fn new(uart: &str, mut handles: Vec<ModemCommsHandle>) -> Result<Self> {
        let mut settings: SerialPortSettings = Default::default();
        // TODO: Should be configurable settings
        settings.timeout = Duration::from_millis(50);
        settings.baud_rate = 230_400;

        let mut port = match serialport::open_with_settings(uart, &settings) {
            Ok(port) => port,
            Err(e) => {
                eprintln!("Failed to open \"{}\". Error: {}", uart, e);
                ::std::process::exit(1);
            }
        };

        let mut map = HashMap::new();
        for handle in handles.drain(..) {
            map.insert(handle.pipe, handle);
        }

        Ok(Self {
            port,
            map,
            cobs_buf: Buffer::new(),
        })
    }

    pub fn poll(&mut self) -> Result<()> {
        let mut raw_buf = [0u8; 256];
        match self.port.read(&mut raw_buf) {
            Ok(ct) => {
                let mut window = &raw_buf[..ct];

                'cobs: while !window.is_empty() {
                    use FeedResult::*;
                    window = match self.cobs_buf.feed::<ModemToPc>(&window) {
                        Consumed => break 'cobs,
                        OverFull(new_wind) => new_wind,
                        DeserError(new_wind) => new_wind,
                        Success { data, remaining } => {
                            match data {
                                ModemToPc::Incoming { pipe, msg } => {
                                    let comms = self.map.get_mut(&pipe).ok_or(String::from("oh."))?;
                                    comms.tx.send(msg)?;
                                }
                            }
                            remaining
                        }
                    };
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                // print!(".");
                io::stdout().flush().ok().expect("Could not flush stdout");
            }
            Err(e) => {
                eprintln!("{:?}", e);
                return Err(String::from("BAD SERIAL ERROR").into());
            }
        }

        for (pipe, comms) in self.map.iter_mut() {
            while let Ok(msg) = comms.rx.try_recv() {
                let msg = PcToModem::Outgoing {
                    msg,
                    pipe: *pipe,
                };
                if let Ok(slice) = postcard::to_slice_cobs(&msg, &mut raw_buf) {
                    println!("Sending {:?}", &msg);
                    self.port.write_all(slice).map_err(drop).ok();
                    self.port.flush().map_err(drop).ok();
                }
            }
        }

        Ok(())

    }
}

