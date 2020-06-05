#![no_std]

#[cfg(feature = "51")]
use nrf51_hal as hal;
#[cfg(feature = "52810")]
use nrf52810_hal as hal;
#[cfg(feature = "52832")]
use nrf52832_hal as hal;
#[cfg(feature = "52840")]
use nrf52840_hal as hal;

use chacha20poly1305::aead::{Buffer, Error as AeadError};

use core::cmp::min;

use serde::de::DeserializeOwned;
use esb::Error as EsbError;
use postcard::Error as PostcardError;

pub mod nonce;
pub mod prx;
pub mod ptx;

#[derive(Debug)]
pub enum Error {
    PacketTooSmol,
    BadNonce,
    InvalidNonce,

    Esb(EsbError),
    Postcard(PostcardError),
    Crypt(AeadError),
}

impl From<EsbError> for Error {
    fn from(err: EsbError) -> Self {
        Error::Esb(err)
    }
}

impl From<PostcardError> for Error {
    fn from(err: PostcardError) -> Self {
        Error::Postcard(err)
    }
}

impl From<AeadError> for Error {
    fn from(err: AeadError) -> Self {
        Error::Crypt(err)
    }
}

struct LilBuf<'a> {
    buf: &'a mut [u8],
    used: u8,
}

#[derive(Debug)]
pub struct RxMessage<T>
where
    T: DeserializeOwned,
{
    pub msg: T,
    pub meta: MessageMetadata
}

#[derive(Debug)]
pub struct MessageMetadata {
    pub pipe: u8,
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
        debug_assert!(len <= 255, "trunc too big");

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

pub const NONCE_SIZE: usize = 12;
pub const CRYPT_SIZE: usize = 16;
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
