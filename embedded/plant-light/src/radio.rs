use esb::{
    consts::*,
    EsbApp,
    ArrayLength,
    EsbHeader,
};

use serde::{Serialize, de::DeserializeOwned};
use core::marker::PhantomData;

use chacha20poly1305::ChaCha8Poly1305; // Or `XChaCha20Poly1305`
use chacha20poly1305::aead::{
    Aead, NewAead, Buffer, Error as AeadError,
    generic_array::{GenericArray, typenum::U128},
};

use crate::hal::Rng;
use core::cmp::min;

use postcard::{
    to_slice,
    from_bytes,
};

struct LilBuf<'a> {
    buf: &'a mut [u8],
    used: u8,
}

impl<'a> AsRef<[u8]> for LilBuf<'a> {
    fn as_ref(&self) -> &[u8] {
        &self.buf[..self.used.into()]
    }
}

impl<'a> AsMut<[u8]> for LilBuf<'a> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.used.into()]
    }
}

impl<'a> Buffer for LilBuf<'a> {
    fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), AeadError> {
        let used_usize = usize::from(self.used);
        let new_used_usize = used_usize + other.len();

        if new_used_usize > min(255, self.buf.len()) {
            return Err(AeadError);
        }
        self.buf[used_usize..new_used_usize].copy_from_slice(other);
        self.used = new_used_usize as u8;
        Ok(())
    }

    fn truncate(&mut self, len: usize) {
        debug_assert!(len <= self.used.into(), "over trunc");
        debug_assert!(255 <= len, "trunc too big");

        let new_used = min(len, 255);
        let new_used = min(new_used, self.used.into());

        self.used = new_used as u8;
    }

    fn len(&self) -> usize {
        self.used.into()
    }

    fn is_empty(&self) -> bool {
        self.used == 0
    }
}



//                            vvvvv    - magic
pub const MAGIC_WORD: u32 = 0xF1337001;
//                                 ^^^ - protocol version
//                                 ^   - major
//                                  ^  - minor
//                                   ^ - trivial

pub const NONCE_SIZE: usize = 16;
pub const CRYPT_SIZE: usize = 12;
pub const MIN_CRYPT_SIZE: usize = NONCE_SIZE + CRYPT_SIZE;

/// This trait decribes a monotonically incrementing timer that
/// is expected to roll over.
///
/// It should be fed with something that changes reasonably often,
/// such as milliseconds, RTC ticks, cycle counts, etc. You should
/// choose something that fulfills both of these criteria:
///
/// * Should tick at least once per few thousand messages
/// * Should not tick so fast to roll over per few messages
pub trait RollingTimer {
    /// Get the current unitless tick
    fn get_current_tick(&self) -> u32;
}

struct FleetRadioPtx<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy, Tick>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    OutgoingTy: Serialize,
    IncomingTy: DeserializeOwned,
    Tick: RollingTimer,
{
    app: EsbApp<OutgoingLen, IncomingLen>,
    crypt: ChaCha8Poly1305,

    tick: Tick,
    tick_window: u32,
    tick_offset: u32,
    last_rx_tick: u32,

    msg_count: u32,
    last_rx_count: u32,

    _ot: PhantomData<OutgoingTy>,
    _it: PhantomData<IncomingTy>,
}

impl<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy, Tick> FleetRadioPtx<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy, Tick>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    OutgoingTy: Serialize,
    IncomingTy: DeserializeOwned,
    Tick: RollingTimer,
{
    pub fn new(
        app: EsbApp<OutgoingLen, IncomingLen>,
        key: &[u8; 16],
        tick: Tick,
        tick_window: u32,
        rng: &mut Rng,
    ) -> Self {
        let ga_key = GenericArray::clone_from_slice(key);
        let crypt = ChaCha8Poly1305::new(ga_key);

        let msg_count = rng.random_u32();
        let tick_offset =  rng.random_u32();

        Self {
            app,
            tick_window,
            tick,
            crypt,

            tick_offset,
            msg_count,

            last_rx_count: msg_count,
            last_rx_tick: tick_offset,

            _ot: PhantomData,
            _it: PhantomData,
        }
    }

    pub fn send(&mut self, msg: &OutgoingTy) -> Result<(), ()> {
        let header = EsbHeader::build()
            .max_payload(252) // toto
            .pid(0) // todo
            .pipe(0) // todo
            .no_ack(false)
            .check()
            .map_err(drop)?;

        let mut grant = self.app.grant_packet(header)
            .map_err(drop)?;

        // serialize directly to buffer
        let used = to_slice(msg, &mut grant)
            .map_err(drop)?
            .len();

        // Update nonce vars
        self.msg_count = self.msg_count.wrapping_add(1);
        let tick = self.current_tick();

        // Create nonce
        let nonce = GenericArray::clone_from_slice(
            &FleetNonce { tick, msg_count: self.msg_count }.to_bytes()
        );

        let mut buf = LilBuf {
            buf: &mut grant,
            used: used as u8,
        };

        // Encrypt
        self.crypt.encrypt_in_place(&nonce, b"", &mut buf)
            .map_err(drop)?;

        // Add nonce to payload
        buf.extend_from_slice(&nonce)
            .map_err(drop)?;

        // Extract the bytes used of the LilBuf
        let used = buf.used.into();

        // Commit payload
        grant.commit(used);

        Ok(())
    }

    pub fn current_tick(&self) -> u32 {
        self.tick.get_current_tick().wrapping_add(self.tick_offset)
    }

    pub fn receive(&mut self) -> Result<Option<IncomingTy>, ()> {
        let mut packet = loop {
            match self.app.read_packet() {
                // No packet ready
                None => return Ok(None),

                // Empty ACK, release and get the next packet
                Some(pkt) if pkt.payload_len() == 0 => {
                    pkt.release();
                    continue;
                }

                // We didn't even get enough bytes for the crypto
                // header (and a 1 byte payload). Release packet and
                // return error
                Some(pkt) if pkt.payload_len() <= MIN_CRYPT_SIZE => {
                    pkt.release();
                    return Err(());
                }

                // We got a potentially good one!
                Some(pkt) => {
                    break pkt;
                },
            };
        };

        let len = packet.payload_len();
        let (payload, nonce) = packet.split_at_mut(len - NONCE_SIZE);
        let nonce = match FleetNonce::try_from_bytes(nonce) {
            Ok(n) => n,
            Err(_e) => {
                packet.release();
                return Err(());
            }
        };

        // Nonce check!
        match self.check_nonce(&nonce) {
            Ok(()) => {},
            Err(()) => {
                packet.release();
                return Err(());
            }
        }

        let nonce = GenericArray::clone_from_slice(&nonce.to_bytes());
        let mut buf = LilBuf { used: payload.len() as u8, buf: payload };

        match self.crypt.decrypt_in_place(&nonce, b"", &mut buf) {
            Ok(()) => {},
            Err(_e) => {
                packet.release();
                return Err(());
            }
        }

        let result = from_bytes(buf.as_ref()).map_err(drop);
        packet.release();
        result
    }

    fn check_nonce(&mut self, nonce: &FleetNonce) -> Result<(), ()> {
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
            self.last_rx_tick  = nonce.tick;
            Ok(())
        } else {
            Err(())
        }
    }


}

struct FleetNonce {
    tick: u32,
    msg_count: u32,
}

impl FleetNonce {
    fn to_bytes(&self) -> [u8; NONCE_SIZE] {
        let mut nonce = [0u8; 16];
        nonce[0..4].copy_from_slice(&self.msg_count.to_le_bytes());
        nonce[4..8].copy_from_slice(&self.tick.to_le_bytes());
        // todo: 8..12 bytes?
        nonce[12..16].copy_from_slice(&MAGIC_WORD.to_le_bytes());
        nonce
    }

    fn try_from_bytes(buf: &[u8]) -> Result<Self, ()> {
        if buf.len() != NONCE_SIZE {
            return Err(())
        }

        if &buf[12..16] != &MAGIC_WORD.to_le_bytes() {
            return Err(());
        }

        let mut m_ct_buf = [0u8; 4];
        let mut tick_buf = [0u8; 4];

        m_ct_buf.copy_from_slice(&buf[0..4]);
        tick_buf.copy_from_slice(&buf[4..8]);

        Ok(Self {
            msg_count: u32::from_le_bytes(m_ct_buf),
            tick: u32::from_le_bytes(tick_buf),
        })
    }
}
