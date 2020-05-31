use esb::{
    consts::*,
    EsbApp,
    ArrayLength,
};

use serde::{Serialize, de::DeserializeOwned};
use core::marker::PhantomData;

use chacha20poly1305::ChaCha8Poly1305; // Or `XChaCha20Poly1305`
use chacha20poly1305::aead::{Aead, NewAead};
use chacha20poly1305::aead::generic_array::{GenericArray, typenum::U128};
use chacha20poly1305::aead::heapless::{Vec, consts::*};

struct FleetRadioPtx<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    OutgoingTy: Serialize,
    IncomingTy: DeserializeOwned,
{
    app: EsbApp<OutgoingLen, IncomingLen>,
    _ot: PhantomData<OutgoingTy>,
    _it: PhantomData<IncomingTy>,
}

impl<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy> FleetRadioPtx<OutgoingLen, IncomingLen, OutgoingTy, IncomingTy>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    OutgoingTy: Serialize,
    IncomingTy: DeserializeOwned,
{
    pub fn new(app: EsbApp<OutgoingLen, IncomingLen>, key) -> Self {
        Self {
            app,
            _ot: PhantomData,
            _it: PhantomData,
        }
    }


}
