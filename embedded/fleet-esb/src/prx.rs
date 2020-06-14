use esb::payload::PayloadR;
use esb::{ArrayLength, EsbApp, EsbHeader};

use core::marker::PhantomData;
use serde::{de::DeserializeOwned, Serialize};

use chacha20poly1305::aead::{generic_array::GenericArray, Aead, Buffer, NewAead};
use chacha20poly1305::ChaCha8Poly1305; // Or `XChaCha20Poly1305`

use postcard::{from_bytes, to_slice};

use crate::{
    nonce::FleetNonce, Error, LilBuf, MessageMetadata, RxMessage, MIN_CRYPT_SIZE, NONCE_SIZE,
};

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

    pub fn send(&mut self, msg: &OutgoingTy, pipe: u8) -> Result<(), Error> {
        let header = EsbHeader::build()
            .max_payload(self.app.maximum_payload_size() as u8)
            .pid(0) // todo
            .pipe(pipe)
            .no_ack(false)
            .check()?;

        let mut grant = self.app.grant_packet(header)?;

        // serialize directly to buffer
        let used = to_slice(msg, &mut grant)?.len();

        let nonce_bytes = FleetNonce { tick: self.last_rx_tick, msg_count: self.last_rx_count }.to_bytes();

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

        Ok(())
    }

    pub fn receive(&mut self) -> Result<Option<RxMessage<IncomingTy>>, Error> {
        let mut packet = match self.app.read_packet() {
            // No packet ready
            None => return Ok(None),

            // We got a potentially good one!
            Some(pkt) => pkt,
        };

        let result = self.process_rx_frame(&mut packet);
        packet.release();
        result.map(Option::Some)
    }

    fn process_rx_frame(&mut self, frame: &mut PayloadR<IncomingLen>) -> Result<RxMessage<IncomingTy>, Error> {
        // We didn't even get enough bytes for the crypto
        // header (and a 1 byte payload). Release packet and
        // return error
        if frame.payload_len() <= MIN_CRYPT_SIZE {
            return Err(Error::PacketTooSmol);
        }

        let len = frame.payload_len();
        let (payload, nonce_bytes) = frame.split_at_mut(len - NONCE_SIZE);
        let fleet_nonce = FleetNonce::try_from_bytes(nonce_bytes)?;

        // TODO(AJM): PRX should probably do some kind of nonce validation. For now,
        // just update the tracking variables
        self.last_rx_tick = fleet_nonce.tick;
        self.last_rx_count = fleet_nonce.msg_count;

        let ga_nonce = GenericArray::from_slice(nonce_bytes);
        let mut buf = LilBuf {
            used: payload.len() as u8,
            buf: payload,
        };

        self.crypt.decrypt_in_place(&ga_nonce, b"", &mut buf)?;

        from_bytes(buf.as_ref())
            .map(|pkt| RxMessage { msg: pkt, meta: MessageMetadata { pipe: frame.pipe() }})
            .map_err(|e| e.into())
    }
}
