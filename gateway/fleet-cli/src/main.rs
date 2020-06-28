use chrono::prelude::*;
use chrono::{DateTime, Local, TimeZone};
use postcard::{from_bytes, to_slice_cobs};
use serde::{Deserialize, Serialize};
use serde_json;
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
use structopt::StructOpt;
use toml::{from_str, to_string};
use fleet_icd::{
    consts::*, Buffer, FeedResult,
    radio::{DeviceToHost, HostToDevice, GeneralHostMessage, PlantLightHostMessage, RelayIdx, RelayState},
    modem::{PcToModem, ModemToPc},
};

#[derive(StructOpt, Debug)]
#[structopt(rename_all = "kebab-case")]
enum SubCommands {
    Reset,
    Log,
    Debug,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct DayLog {
    log: HashMap<String, TaskLog>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskLog {
    time_spent: Duration,
    events: Vec<LogEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
enum LogEvent {
    Start(DateTime<Local>),
    End(DateTime<Local>),
}

#[derive(Debug, Serialize, Deserialize)]
struct Task {
    pin: u8,
}

pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

fn main() -> Result<()> {
    let opt = SubCommands::from_args();

    let mut settings: SerialPortSettings = Default::default();
    settings.timeout = Duration::from_millis(50);
    settings.baud_rate = 230_400;

    let mut port = match serialport::open_with_settings("/dev/ttyACM0", &settings) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("Failed to open \"{}\". Error: {}", "/dev/ttyACM0", e);
            ::std::process::exit(1);
        }
    };

    let ret = log(&mut port);

    // let ret = match opt {
    //     SubCommands::Reset => {
    //         reset(&mut port).ok();
    //         sleep(Duration::from_secs(1));
    //         Ok(())
    //     }
    //     SubCommands::Log => log(config, &mut port),
    //     SubCommands::Debug => debug(&mut port),
    // };

    if ret.is_err() {
        println!();
    }

    ret
}

fn reset(port: &mut Box<dyn SerialPort>) -> Result<()> {
    // let mut raw_buf = [0u8; 256];
    // let msg = HostToDeviceMessages::Reset;

    // loop {
    //     if let Ok(slice) = postcard::to_slice_cobs(&msg, &mut raw_buf) {
    //         println!("Reset...");
    //         port.write_all(slice)?;
    //         port.flush()?;
    //     }
    //     sleep(Duration::from_millis(100));
    // }
    todo!()
}

fn log(port: &mut Box<dyn SerialPort>) -> Result<()> {
    let mut cobs_buf: Buffer<U256> = Buffer::new();
    let mut raw_buf = [0u8; 256];
    let mut now = Instant::now();
    let mut msgs = VecDeque::new();
    let mut counter = 0;

    loop {
        match port.read(&mut raw_buf) {
            Ok(ct) => {
                let mut window = &raw_buf[..ct];

                'cobs: while !window.is_empty() {
                    use FeedResult::*;
                    window = match cobs_buf.feed::<ModemToPc>(&window) {
                        Consumed => break 'cobs,
                        OverFull(new_wind) => new_wind,
                        DeserError(new_wind) => new_wind,
                        Success { data, remaining } => {
                            msgs.push_back(data);
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
                return Err(Error::from("BAD SERIAL ERROR"));
            }
        }



        while let Some(msg) = msgs.pop_front() {
            println!("{:?}", msg);
        }

        if now.elapsed() >= Duration::from_millis(3000) {
            let msg = match counter {
                0 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay0,
                        state: RelayState::On,
                    })
                }
                1 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay1,
                        state: RelayState::On,
                    })
                }
                2 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay2,
                        state: RelayState::On,
                    })
                }
                3 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay3,
                        state: RelayState::On,
                    })
                }

                4 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay0,
                        state: RelayState::Off,
                    })
                }
                5 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay1,
                        state: RelayState::Off,
                    })
                }
                6 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay2,
                        state: RelayState::Off,
                    })
                }
                7 => {
                    HostToDevice::PlantLight(PlantLightHostMessage::SetRelay {
                        relay: RelayIdx::Relay3,
                        state: RelayState::Off,
                    })
                }

                _ => {
                    counter = -1;
                    HostToDevice::General(GeneralHostMessage::Ping)
                }
            };

            counter += 1;
            let msg = PcToModem::Outgoing {
                msg,
                pipe: 0,
            };
            if let Ok(slice) = postcard::to_slice_cobs(&msg, &mut raw_buf) {
                println!("Sending {:?}", &msg);
                port.write_all(slice).map_err(drop).ok();
                port.flush().map_err(drop).ok();
                now = Instant::now();
            }
        }
    }
    Ok(())
}

fn debug(port: &mut Box<dyn SerialPort>) -> Result<()> {
    // let mut cobs_buf: Buffer<U256> = Buffer::new();
    // let mut raw_buf = [0u8; 256];
    // let mut now = Instant::now();
    // let mut any_acks = false;
    // let mut panic_once = false;

    // loop {
    //     if now.elapsed() >= Duration::from_millis(500) {
    //         now = Instant::now();

    //         let msg = HostToDeviceMessages::Ping;

    //         if let Ok(slice) = postcard::to_slice_cobs(&msg, &mut raw_buf) {
    //             port.write_all(slice).map_err(drop).ok();
    //             port.flush().map_err(drop).ok();
    //         }

    //         println!("\nSENT: {:?}", msg);
    //     }

    //     if !panic_once && any_acks {
    //         panic_once = true;

    //         let msg = HostToDeviceMessages::GetPanic;

    //         if let Ok(slice) = postcard::to_slice_cobs(&msg, &mut raw_buf) {
    //             port.write_all(slice).map_err(drop).ok();
    //             port.flush().map_err(drop).ok();
    //         }

    //         println!("\nSENT: {:?}", msg);
    //     }

    //     let buf = match port.read(&mut raw_buf) {
    //         Ok(ct) => &raw_buf[..ct],
    //         Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
    //             print!(".");
    //             io::stdout().flush().ok().expect("Could not flush stdout");
    //             continue;
    //         }
    //         Err(e) => {
    //             eprintln!("{:?}", e);
    //             return Err(Error::from("BAD SERIAL ERROR"));
    //         }
    //     };

    //     let mut window = &buf[..];

    //     'cobs: while !window.is_empty() {
    //         use FeedResult::*;
    //         window = match cobs_buf.feed::<DeviceToHostMessages>(&window) {
    //             Consumed => break 'cobs,
    //             OverFull(new_wind) => new_wind,
    //             DeserError(new_wind) => new_wind,
    //             Success { data, remaining } => {
    //                 println!("\nGOT: {:?}", data);
    //                 if data == DeviceToHostMessages::Ack {
    //                     any_acks = true;
    //                 }
    //                 remaining
    //             }
    //         };
    //     }
    // }
    todo!()
}
