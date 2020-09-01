use crate::{Route, Result, HomeFleetTable};
use serialport::prelude::*;
use std::{
    io::prelude::*,
    time::Duration,
};

pub struct CommsCtx {
    routes: Vec<Route>,
    uart: UartAnachro,
    client: Client,
}

use anachro_icd::{
    arbitrator::Arbitrator,
    component::Component,
    Version,
};
use anachro_client::{ClientIo, ClientError, Client, Error};
use postcard::{from_bytes_cobs, to_stdvec_cobs};

struct UartAnachro {
    port: Box<dyn SerialPort>,
    scratch: Vec<u8>,
    current: Option<Vec<u8>>
}


impl ClientIo for UartAnachro {
    fn recv(&mut self) -> core::result::Result<Option<Arbitrator>, ClientError> {
        let mut scratch = [0u8; 1024];

        loop {
            match self.port.read(&mut scratch) {
                Ok(n) if n > 0 => {
                    self.scratch.extend_from_slice(&scratch[..n]);

                    if let Some(p) = self.scratch.iter().position(|c| *c == 0x00) {
                        let mut remainder = self.scratch.split_off(p + 1);
                        core::mem::swap(&mut remainder, &mut self.scratch);
                        self.current = Some(remainder);

                        if let Some(ref mut payload) = self.current {
                            if let Ok(msg) = from_bytes_cobs::<Arbitrator>(payload.as_mut_slice()) {
                                println!("GIVING: {:?}", msg);
                                return Ok(Some(msg));
                            }
                        }

                        return Err(ClientError::ParsingError);
                    }
                }
                Ok(_) => return Ok(None),
                Err(_) => return Ok(None),
            }
        }
    }
    fn send(&mut self, msg: &Component) -> core::result::Result<(), ClientError> {
        println!("SENDING: {:?}", msg);
        let ser = to_stdvec_cobs(msg).map_err(|_| ClientError::ParsingError)?;
        self.port
            .write_all(&ser)
            .map_err(|_| ClientError::OutputFull)?;
        Ok(())
    }
}

impl CommsCtx {
    pub fn new(uart: &str, routes: Vec<Route>) -> Result<Self> {
        let mut settings: SerialPortSettings = Default::default();
        // TODO: Should be configurable settings
        settings.timeout = Duration::from_millis(50);
        settings.baud_rate = 230_400;

        let port = match serialport::open_with_settings(uart, &settings) {
            Ok(port) => port,
            Err(e) => {
                eprintln!("Failed to open \"{}\". Error: {}", uart, e);
                ::std::process::exit(1);
            }
        };

        let client = Client::new(
            "fleet-manager",
            Version {
                major: 0,
                minor: 4,
                trivial: 1,
                misc: 123,
            },
            987,
            HomeFleetTable::sub_paths(),
            HomeFleetTable::pub_paths(),
            Some(100),
        );

        Ok(Self {
            uart: UartAnachro {
                port,
                scratch: vec![],
                current: None,
            },
            routes,
            client,
        })
    }

    pub fn poll(&mut self) -> Result<()> {

        loop {
            let msg = match self.client.process_one::<_, HomeFleetTable>(&mut self.uart) {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    break;
                }
                Err(Error::ClientIoError(ClientError::NoData)) => {
                    break;
                }
                Err(e) => {
                    println!("error: {:?}", e);
                    return Err("errr oohhh".into());
                },
            };

            for route in self.routes.iter_mut() {
                for path in route.paths.iter() {
                    if msg.path.as_str() == *path {
                        route.comms.tx.send(msg.payload.clone()).ok();
                    }
                }
            }
        }

        if self.client.is_connected() {
            for route in self.routes.iter_mut() {
                while let Ok(msg) = route.comms.rx.try_recv() {
                    let mut buf = [0u8; 1024];
                    let pubby = msg.serialize(&mut buf).map_err(|_| "arg")?;
                    self.client.publish(
                        &mut self.uart,
                        pubby.path,
                        pubby.buf,
                    ).map_err(|_| "blarg")?;
                }
            }
        }


        Ok(())
    }
}
