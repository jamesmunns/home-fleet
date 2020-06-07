#![no_std]

pub mod keys;

pub struct FleetKey {
    pub(crate) key: [u8; 32],
}

impl FleetKey {
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }
}
