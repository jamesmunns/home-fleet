use crate::{Error, MAGIC_WORD, NONCE_SIZE};

pub struct FleetNonce {
    pub(crate) tick: u32,
    pub(crate) msg_count: u32,
}

impl FleetNonce {
    pub fn to_bytes(&self) -> [u8; NONCE_SIZE] {
        let mut nonce = [0u8; NONCE_SIZE];
        nonce[0..4].copy_from_slice(&self.msg_count.to_le_bytes());
        nonce[4..8].copy_from_slice(&self.tick.to_le_bytes());
        nonce[8..12].copy_from_slice(&MAGIC_WORD.to_le_bytes());
        nonce
    }

    pub fn try_from_bytes(buf: &[u8]) -> Result<Self, Error> {
        if buf.len() != NONCE_SIZE {
            return Err(Error::BadNonce);
        }

        if buf[8..12] != MAGIC_WORD.to_le_bytes() {
            return Err(Error::BadNonce);
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
