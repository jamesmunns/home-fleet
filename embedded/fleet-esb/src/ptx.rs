pub use esb::payload::PayloadR;
use esb::{ArrayLength, EsbApp, EsbHeader};

use serde::{de::DeserializeOwned, Serialize};

use chacha20poly1305::aead::{generic_array::GenericArray, Aead, Buffer, NewAead};
use chacha20poly1305::ChaCha8Poly1305; // Or `XChaCha20Poly1305`

use crate::hal::Rng;

use postcard::{from_bytes, to_slice};

use crate::{
    nonce::FleetNonce, BorrowRxMessage, Error, LilBuf, MessageMetadata, RollingTimer, RxMessage,
    MIN_CRYPT_SIZE, NONCE_SIZE,
};

use serde::de::Deserialize;

pub struct FleetRadioPtx<OutgoingLen, IncomingLen, Tick>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    Tick: RollingTimer,
{
    app: EsbApp<OutgoingLen, IncomingLen>,
    crypt: ChaCha8Poly1305,
    tick: Tick,

    tick_window: u32,
    tick_offset: u32,
    last_tx_tick: u32,
    last_rx_tick: u32,

    msg_count: u32,
    last_rx_count: u32,
}

impl<OutgoingLen, IncomingLen, Tick> FleetRadioPtx<OutgoingLen, IncomingLen, Tick>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    Tick: RollingTimer,
{
    pub fn new(
        app: EsbApp<OutgoingLen, IncomingLen>,
        key: &[u8; 32],
        tick: Tick,
        tick_window: u32,
        rng: &mut Rng,
    ) -> Self {
        let ga_key = GenericArray::clone_from_slice(key);
        let crypt = ChaCha8Poly1305::new(ga_key);

        let msg_count = rng.random_u32();
        let tick_offset = rng.random_u32();

        Self {
            app,
            tick_window,
            tick,
            crypt,

            tick_offset,
            msg_count,

            last_rx_count: msg_count,
            last_rx_tick: tick_offset,
            last_tx_tick: tick_offset,
        }
    }

    pub fn send<T: Serialize>(&mut self, msg: &T, pipe: u8) -> Result<(), Error> {
        let header = EsbHeader::build()
            .max_payload(self.app.maximum_payload_size() as u8)
            .pid(0) // todo
            .pipe(pipe)
            .no_ack(false)
            .check()?;

        let mut grant = self.app.grant_packet(header)?;

        // serialize directly to buffer
        let used = to_slice(msg, &mut grant)?.len();

        // Update nonce vars
        self.msg_count = self.msg_count.wrapping_add(1);
        let tick = self.current_tick();

        let nonce_bytes = FleetNonce {
            tick,
            msg_count: self.msg_count,
        }
        .to_bytes();

        // Create nonce
        let ga_nonce = GenericArray::from_slice(&nonce_bytes);

        let mut buf = LilBuf {
            buf: &mut grant,
            used: used as u8,
        };

        // Encrypt
        self.crypt.encrypt_in_place(&ga_nonce, b"", &mut buf)?;

        // Add nonce to payload
        buf.extend_from_slice(&ga_nonce)?;

        // Extract the bytes used of the LilBuf
        let used = buf.used.into();

        // Commit payload
        grant.commit(used);

        // Update TX
        self.last_tx_tick = tick;

        // Kick the radio
        self.app.start_tx();

        Ok(())
    }

    pub fn current_tick(&self) -> u32 {
        self.tick.get_current_tick().wrapping_add(self.tick_offset)
    }

    pub fn ticks_since_last_tx(&self) -> u32 {
        self.current_tick().wrapping_sub(self.last_tx_tick)
    }

    pub fn ticks_since_last_rx(&self) -> u32 {
        self.current_tick().wrapping_sub(self.last_rx_tick)
    }

    pub fn receive<T: 'static + DeserializeOwned>(
        &mut self,
    ) -> Result<Option<RxMessage<T>>, Error> {
        let mut with_rgr = match self.receive_with() {
            Ok(rgr) => rgr,
            Err(Error::NoData) => return Ok(None),
            Err(e) => return Err(e),
        };

        let view = with_rgr.view_with(|msg: BorrowRxMessage<T>| RxMessage {
            msg: msg.msg,
            meta: msg.meta,
        });

        // TODO - Auto Release?
        with_rgr.fgr.release();

        match view {
            Ok(msg) => Ok(Some(msg)),
            Err(e) => Err(e),
        }
    }

    pub fn just_gimme_frame(&mut self) -> Result<PayloadR<IncomingLen>, Error> {
        let mut frame = loop {
            let pkt = self.app.read_packet().map(|mut frame| {
                frame.auto_release(true);
                frame
            });

            match pkt {
                // No packet ready
                None => return Err(Error::NoData),

                // Empty ACK, release and get the next packet
                Some(pkt) if pkt.payload_len() == 0 => {
                    continue;
                }

                // We got a potentially good one!
                Some(pkt) => {
                    break pkt;
                }
            };
        };

        if frame.payload_len() <= MIN_CRYPT_SIZE {
            return Err(Error::PacketTooSmol);
        }

        let len = frame.payload_len();
        let (payload, nonce_bytes) = frame.split_at_mut(len - NONCE_SIZE);
        let fleet_nonce = FleetNonce::try_from_bytes(nonce_bytes)?;

        // Nonce check!
        self.check_nonce_and_update(&fleet_nonce)?;

        let ga_nonce = GenericArray::from_slice(nonce_bytes);
        let mut buf = LilBuf {
            used: payload.len() as u8,
            buf: payload,
        };

        self.crypt.decrypt_in_place(ga_nonce, b"", &mut buf)?;

        Ok(frame)
    }

    pub fn receive_with(&mut self) -> Result<GrantWrap<IncomingLen>, Error> {
        let frame = self.just_gimme_frame()?;
        Ok(GrantWrap { fgr: frame })
    }

    fn check_nonce_and_update(&mut self, nonce: &FleetNonce) -> Result<(), Error> {
        let cur_tick = self.current_tick();
        let min_tick = cur_tick.wrapping_sub(self.tick_window);

        // Is the tick >= the last received tick?
        let last_tick_good = if self.last_rx_tick > cur_tick {
            // We have rolled. Make sure the tick is between
            // last and u32::max, OR between zero and current
            (nonce.tick >= self.last_rx_tick) || (nonce.tick <= cur_tick)
        } else {
            (nonce.tick >= self.last_rx_tick) && (nonce.tick <= cur_tick)
        };

        // Is the tick >= our minimum acceptable staleness?
        let min_tick_good = if min_tick > cur_tick {
            // We have rolled. Make sure the tick is between
            // min and u32::max, OR between zero and current
            (nonce.tick >= min_tick) || (nonce.tick <= cur_tick)
        } else {
            (nonce.tick >= min_tick) && (nonce.tick <= cur_tick)
        };

        // Is the count >= the last received count?
        let count_good = if self.last_rx_count > cur_tick {
            // We have rolled. Blah Blah
            (nonce.msg_count >= self.last_rx_count) || (nonce.msg_count <= self.msg_count)
        } else {
            (nonce.msg_count >= self.last_rx_count) && (nonce.msg_count <= self.msg_count)
        };

        if last_tick_good && min_tick_good && count_good {
            // Success, update tracking variables
            self.last_rx_count = nonce.msg_count;
            self.last_rx_tick = nonce.tick;
            Ok(())
        } else {
            Err(Error::InvalidNonce)
        }
    }
}

pub struct GrantWrap<N>
where
    N: ArrayLength<u8>,
{
    fgr: PayloadR<N>,
}

impl<N> GrantWrap<N>
where
    N: ArrayLength<u8>,
{
    pub fn view_with<'a: 'de, 'de, T, F, R>(&'a mut self, fun: F) -> Result<R, Error>
    where
        T: 'de + Deserialize<'de>,
        F: FnOnce(BorrowRxMessage<'de, T>) -> R,
        R: 'static,
    {
        match process_rx_frame::<T, N>(&mut self.fgr) {
            Ok(msg) => Ok(fun(msg)),
            Err(e) => Err(e),
        }
    }
}

fn process_rx_frame<'de, T: 'de + Deserialize<'de>, N>(
    frame: &'de mut PayloadR<N>,
) -> Result<BorrowRxMessage<'de, T>, Error>
where
    N: ArrayLength<u8>,
{
    let len = frame.payload_len();
    let payload_len = len - NONCE_SIZE;

    from_bytes(&frame[..payload_len])
        .map(|pkt| BorrowRxMessage {
            msg: pkt,
            meta: MessageMetadata { pipe: frame.pipe() },
            marker: core::marker::PhantomData,
        })
        .map_err(|e| e.into())
}
