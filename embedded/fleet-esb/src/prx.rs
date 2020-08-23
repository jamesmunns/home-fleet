use esb::payload::PayloadR;
use esb::{ArrayLength, EsbApp, EsbHeader};

use serde::{
    de::{Deserialize, DeserializeOwned},
    Serialize,
};

use chacha20poly1305::aead::{generic_array::GenericArray, Aead, Buffer, NewAead};
use chacha20poly1305::ChaCha8Poly1305; // Or `XChaCha20Poly1305`

use postcard::{from_bytes, to_slice};

use crate::{
    nonce::FleetNonce, BorrowRxMessage, Error, LilBuf, MessageMetadata, RxMessage, MIN_CRYPT_SIZE,
    NONCE_SIZE,
};

pub struct GrantWrap<N>
where
    N: ArrayLength<u8>,
{
    // TODO
    pub fgr: PayloadR<N>,
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
    let pipe = frame.pipe();
    let len = frame.payload_len();
    let payload_len = len - NONCE_SIZE;

    from_bytes(&frame[..payload_len])
        .map(|pkt| BorrowRxMessage {
            msg: pkt,
            meta: MessageMetadata { pipe },
            marker: core::marker::PhantomData,
        })
        .map_err(|e| e.into())
}

pub struct FleetRadioPrx<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    app: EsbApp<OutgoingLen, IncomingLen>,
    crypt: ChaCha8Poly1305,

    last_rx_tick: u32,
    last_rx_count: u32,
}

impl<OutgoingLen, IncomingLen> FleetRadioPrx<OutgoingLen, IncomingLen>
where
    OutgoingLen: 'static + ArrayLength<u8>,
    IncomingLen: 'static + ArrayLength<u8>,
{
    pub fn new(app: EsbApp<OutgoingLen, IncomingLen>, key: &[u8; 32]) -> Self {
        let ga_key = GenericArray::clone_from_slice(key);
        let crypt = ChaCha8Poly1305::new(ga_key);

        Self {
            app,
            crypt,

            last_rx_count: 0,
            last_rx_tick: 0,
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

        let nonce_bytes = FleetNonce {
            tick: self.last_rx_tick,
            msg_count: self.last_rx_count,
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

        Ok(())
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

    pub fn receive_with(&mut self) -> Result<GrantWrap<IncomingLen>, Error> {
        let mut frame = loop {
            match self.app.read_packet() {
                // No packet ready
                None => return Err(Error::NoData),

                // Empty ACK, release and get the next packet
                Some(pkt) if pkt.payload_len() == 0 => {
                    pkt.release();
                    continue;
                }

                // We got a potentially good one!
                Some(pkt) => {
                    break pkt;
                }
            };
        };

        // We didn't even get enough bytes for the crypto
        // header (and a 1 byte payload). Release packet and
        // return error
        if frame.payload_len() <= MIN_CRYPT_SIZE {
            frame.release();
            return Err(Error::PacketTooSmol);
        }

        let len = frame.payload_len();
        let payload_len = len - NONCE_SIZE;
        let (payload, nonce_bytes) = frame.split_at_mut(payload_len);
        let fleet_nonce = match FleetNonce::try_from_bytes(nonce_bytes) {
            Ok(nonce) => nonce,
            Err(e) => {
                frame.release();
                return Err(e);
            }
        };

        // TODO(AJM): PRX should probably do some kind of nonce validation. For now,
        // just update the tracking variables
        self.last_rx_tick = fleet_nonce.tick;
        self.last_rx_count = fleet_nonce.msg_count;

        let ga_nonce = GenericArray::from_slice(nonce_bytes);
        let mut buf = LilBuf {
            used: payload.len() as u8,
            buf: payload,
        };

        match self.crypt.decrypt_in_place(&ga_nonce, b"", &mut buf) {
            Ok(_) => {}
            Err(e) => {
                frame.release();
                return Err(e.into());
            }
        };

        Ok(GrantWrap { fgr: frame })
    }
}
