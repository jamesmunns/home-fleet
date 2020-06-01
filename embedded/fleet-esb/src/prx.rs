use esb::{ArrayLength, EsbApp, EsbHeader};

use core::marker::PhantomData;
use serde::{de::DeserializeOwned, Serialize};

use chacha20poly1305::aead::{generic_array::GenericArray, Aead, Buffer, NewAead};
use chacha20poly1305::ChaCha8Poly1305; // Or `XChaCha20Poly1305`

use postcard::{from_bytes, to_slice};

use crate::{nonce::FleetNonce, Error, LilBuf, MIN_CRYPT_SIZE, NONCE_SIZE};

pub struct FleetRadioPrx<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    OutgoingTy: Serialize,
    IncomingTy: DeserializeOwned,
{
    app: EsbApp<OutgoingLen, IncomingLen>,
    crypt: ChaCha8Poly1305,

    last_rx_tick: u32,
    last_rx_count: u32,

    _ot: PhantomData<OutgoingTy>,
    _it: PhantomData<IncomingTy>,
}

impl<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy>
    FleetRadioPrx<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    OutgoingTy: Serialize,
    IncomingTy: DeserializeOwned,
{
    pub fn new(app: EsbApp<OutgoingLen, IncomingLen>, key: &[u8; 32]) -> Self {
        let ga_key = GenericArray::clone_from_slice(key);
        let crypt = ChaCha8Poly1305::new(ga_key);

        Self {
            app,
            crypt,

            last_rx_count: 0,
            last_rx_tick: 0,

            _ot: PhantomData,
            _it: PhantomData,
        }
    }

    pub fn send(&mut self, msg: &OutgoingTy) -> Result<(), Error> {
        let header = EsbHeader::build()
            .max_payload(self.app.maximum_payload_size() as u8)
            .pid(0) // todo
            .pipe(0) // todo
            .no_ack(false)
            .check()
            .map_err(|_| Error::HeaderError)?;

        let mut grant = self
            .app
            .grant_packet(header)
            .map_err(|_| Error::QueueFull)?;

        // serialize directly to buffer
        let used = to_slice(msg, &mut grant).map_err(|_| Error::Ser)?.len();

        // Create nonce
        let nonce = GenericArray::clone_from_slice(
            &FleetNonce {
                tick: self.last_rx_tick,
                msg_count: self.last_rx_count,
            }
            .to_bytes(),
        );

        let mut buf = LilBuf {
            buf: &mut grant,
            used: used as u8,
        };

        // Encrypt
        self.crypt
            .encrypt_in_place(&nonce, b"", &mut buf)
            .map_err(|_| Error::Encrypt)?;

        // Add nonce to payload
        buf.extend_from_slice(&nonce)
            .map_err(|_| Error::BufferTooSmol)?;

        // Extract the bytes used of the LilBuf
        let used = buf.used.into();

        // Commit payload
        grant.commit(used);

        Ok(())
    }

    pub fn receive(&mut self) -> Result<Option<IncomingTy>, Error> {
        let mut packet = match self.app.read_packet() {
            // No packet ready
            None => return Ok(None),

            // We didn't even get enough bytes for the crypto
            // header (and a 1 byte payload). Release packet and
            // return error
            Some(pkt) if pkt.payload_len() <= MIN_CRYPT_SIZE => {
                pkt.release();
                return Err(Error::PacketTooSmol);
            }

            // We got a potentially good one!
            Some(pkt) => pkt,
        };

        let len = packet.payload_len();
        let (payload, nonce) = packet.split_at_mut(len - NONCE_SIZE);
        let nonce = match FleetNonce::try_from_bytes(nonce) {
            Ok(n) => n,
            Err(_e) => {
                packet.release();
                return Err(Error::BadNonce);
            }
        };

        // TODO(AJM): PRX should probably do some kind of nonce validation. For now,
        // just update the tracking variables
        self.last_rx_tick = nonce.tick;
        self.last_rx_count = nonce.msg_count;

        let nonce = GenericArray::clone_from_slice(&nonce.to_bytes());
        let mut buf = LilBuf {
            used: payload.len() as u8,
            buf: payload,
        };

        match self.crypt.decrypt_in_place(&nonce, b"", &mut buf) {
            Ok(()) => {}
            Err(_e) => {
                packet.release();
                return Err(Error::Decrypt);
            }
        }

        let result = match from_bytes::<IncomingTy>(buf.as_ref()) {
            Ok(deser) => Ok(Some(deser)),
            Err(_e) => Err(Error::Deser),
        };

        packet.release();

        result
    }

    // fn check_nonce_and_update(&mut self, nonce: &FleetNonce) -> Result<(), ()> {
    //     let cur_tick = self.current_tick();
    //     let min_tick = cur_tick.wrapping_sub(self.tick_window);

    //     // Is the tick >= the last received tick?
    //     let last_tick_good = if self.last_rx_tick > cur_tick {
    //         // We have rolled. Make sure the tick is between
    //         // last and u32::max, OR between zero and current
    //         (nonce.tick >= self.last_rx_tick) || (nonce.tick <= cur_tick)
    //     } else {
    //         (nonce.tick >= self.last_rx_tick) && (nonce.tick <= cur_tick)
    //     };

    //     // Is the tick >= our minimum acceptable staleness?
    //     let min_tick_good = if min_tick > cur_tick {
    //         // We have rolled. Make sure the tick is between
    //         // min and u32::max, OR between zero and current
    //         (nonce.tick >= min_tick) || (nonce.tick <= cur_tick)
    //     } else {
    //         (nonce.tick >= min_tick) && (nonce.tick <= cur_tick)
    //     };

    //     // Is the count >= the last received count?
    //     let count_good = if self.last_rx_count > cur_tick {
    //         // We have rolled. Blah Blah
    //         (nonce.msg_count >= self.last_rx_count) || (nonce.msg_count <= self.msg_count)
    //     } else {
    //         (nonce.msg_count >= self.last_rx_count) && (nonce.msg_count <= self.msg_count)
    //     };

    //     if last_tick_good && min_tick_good && count_good {
    //         // Success, update tracking variables
    //         self.last_rx_count = nonce.msg_count;
    //         self.last_rx_tick  = nonce.tick;
    //         Ok(())
    //     } else {
    //         Err(())
    //     }
    // }
}
