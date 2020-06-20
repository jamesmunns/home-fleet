use bbqueue::{ArrayLength, Consumer, Producer};

pub struct UarteApp<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub(crate) outgoing_prod: Producer<'static, OutgoingLen>,
    pub incoming_cons: Consumer<'static, IncomingLen>,
}

impl<OutgoingLen, IncomingLen> UarteApp<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
}
