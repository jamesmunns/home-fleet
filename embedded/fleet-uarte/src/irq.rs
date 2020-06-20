use crate::hal::timer::Instance as TimerInstance;
use crate::hal::uarte::Instance as UarteInstance;
use core::sync::atomic::{AtomicBool, compiler_fence, Ordering::SeqCst};
use crate::hal::pac::{UARTE0, NVIC, Interrupt};
use bbqueue::{Producer, Consumer, ArrayLength, GrantR, GrantW};
use crate::hal::uarte::{
    Pins,
    Parity,
    Baudrate,
};
use embedded_hal::digital::v2::OutputPin;

pub struct UarteTimer<Timer>
where
    Timer: TimerInstance
{
    pub(crate) timer: Timer,
    pub(crate) timeout_flag: &'static AtomicBool,
}

impl<Timer> UarteTimer<Timer>
where
    Timer: TimerInstance
{
    pub fn init(&mut self, microsecs: u32) {
        self.timer.disable_interrupt();
        self.timer.timer_cancel();
        self.timer.set_periodic();
        self.timer.set_shorts_periodic();
        self.timer.enable_interrupt();

        self.timer.timer_start(microsecs);
    }

    pub fn interrupt(&self) {
        // pend uarte interrupt
        // TODO: Don't hardcode UARTE0
        self.timer.timer_reset_event();
        self.timeout_flag.store(true, SeqCst);
        NVIC::pend(Interrupt::UARTE0_UART0);
    }
}


pub struct UarteIrq<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub(crate) outgoing_cons: Consumer<'static, OutgoingLen>,
    pub(crate) incoming_prod: Producer<'static, IncomingLen>,
    pub(crate) timeout_flag: &'static AtomicBool,
    pub(crate) rx_grant: Option<GrantW<'static, IncomingLen>>,
    pub(crate) tx_grant: Option<GrantR<'static, OutgoingLen>>,
    pub(crate) uarte: UARTE0,
}

use rtt_target::rprintln;

impl<OutgoingLen, IncomingLen> UarteIrq<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub fn init(&mut self, pins: Pins, parity: Parity, baudrate: Baudrate) {
        uarte_setup(&self.uarte, pins, parity, baudrate);
        if let Ok(mut gr) = self.incoming_prod.grant_exact(32) {
            uarte_start_read(&self.uarte, &mut gr).unwrap();
            self.rx_grant = Some(gr);
        }
    }

    pub fn interrupt(&mut self) -> usize {
        let endrx = self.uarte.events_endrx.read().bits() != 0;
        let endtx = self.uarte.events_endtx.read().bits() != 0;
        let error = self.uarte.events_error.read().bits() != 0;

        // let cts = self.uarte.events_cts.read().bits() != 0;
        // let ncts = self.uarte.events_ncts.read().bits() != 0;
        let rxdrdy = self.uarte.events_rxdrdy.read().bits() != 0;
        // let rxstarted = self.uarte.events_rxstarted.read().bits() != 0;

        // let rxto = self.uarte.events_rxto.read().bits() != 0;
        // let txdrdy = self.uarte.events_txdrdy.read().bits() != 0;
        // let txstarted = self.uarte.events_txstarted.read().bits() != 0;
        // let txstopped = self.uarte.events_txstopped.read().bits() != 0;

        // rprintln!("->UINT1 {} {} {}", endrx, endtx, error);
        // rprintln!("->UINT2 {} {} {} {}", cts, ncts, rxdrdy, rxstarted);
        // rprintln!("->UINT3 {} {} {} {}", rxto, txdrdy, txstarted, txstopped);

        // why?
        if !endrx && self.timeout_flag.swap(false, SeqCst) && rxdrdy {
            // rprintln!("-->CANCEL");
            uarte_cancel_read(&self.uarte);
            uarte_finalize_read(&self.uarte);
        }
        compiler_fence(SeqCst);

        let amt = if rxdrdy {
            self.uarte.rxd.amount.read().bits() as usize
        } else {
            0
        };

        if amt != 0 {
            let gr = self.rx_grant.take();

            if let Some(gr) = gr {
                // rprintln!("-->COMMIT");
                gr.commit(amt);
            }

            if let Ok(mut gr) = self.incoming_prod.grant_exact(32) {
                // rprintln!("-->START");
                uarte_start_read(&self.uarte, &mut gr).unwrap();
                self.rx_grant = Some(gr);
            } else {
                // rprintln!("uhhh....")
            }
        }

        compiler_fence(SeqCst);

        self.uarte.events_endrx.write(|w| w);
        self.uarte.events_endtx.write(|w| w);
        self.uarte.events_error.write(|w| w);

        self.uarte.events_cts.write(|w| w);
        self.uarte.events_ncts.write(|w| w);
        self.uarte.events_rxdrdy.write(|w| w);
        self.uarte.events_rxstarted.write(|w| w);

        self.uarte.events_rxto.write(|w| w);
        self.uarte.events_txdrdy.write(|w| w);
        self.uarte.events_txstarted.write(|w| w);
        self.uarte.events_txstopped.write(|w| w);

        self.uarte.errorsrc.modify(|_r, w| w);


        compiler_fence(SeqCst);

        amt
    }
}

/// Start a UARTE read transaction by setting the control
/// values and triggering a read task
fn uarte_start_read(uarte: &UARTE0, rx_buffer: &mut [u8]) -> Result<(), ()> {
    // This is overly restrictive. See (similar SPIM issue):
    // https://github.com/nrf-rs/nrf52/issues/17
    if rx_buffer.len() > u8::max_value() as usize {
        return Err(());
    }

    // NOTE: RAM slice check is not necessary, as a mutable slice can only be
    // built from data located in RAM

    // Conservative compiler fence to prevent optimizations that do not
    // take in to account actions by DMA. The fence has been placed here,
    // before any DMA action has started
    compiler_fence(SeqCst);

    // Set up the DMA read
    uarte.rxd.ptr.write(|w|
        // We're giving the register a pointer to the stack. Since we're
        // waiting for the UARTE transaction to end before this stack pointer
        // becomes invalid, there's nothing wrong here.
        //
        // The PTR field is a full 32 bits wide and accepts the full range
        // of values.
        unsafe { w.ptr().bits(rx_buffer.as_ptr() as u32) });
    uarte.rxd.maxcnt.write(|w|
        // We're giving it the length of the buffer, so no danger of
        // accessing invalid memory. We have verified that the length of the
        // buffer fits in an `u8`, so the cast to `u8` is also fine.
        //
        // The MAXCNT field is at least 8 bits wide and accepts the full
        // range of values.
        unsafe { w.maxcnt().bits(rx_buffer.len() as _) });

    // AJM
    // uarte.events_rxstarted.write(|w| w);

    // Start UARTE Receive transaction
    uarte.tasks_startrx.write(|w|
        // `1` is a valid value to write to task registers.
        unsafe { w.bits(1) });


    // AJM
    // while uarte.events_rxstarted.read().bits() == 0 {}

    Ok(())
}

/// Finalize a UARTE read transaction by clearing the event
fn uarte_finalize_read(uarte: &UARTE0) {
    // Reset the event, otherwise it will always read `1` from now on.
    uarte.events_endrx.write(|w| w);

    // Conservative compiler fence to prevent optimizations that do not
    // take in to account actions by DMA. The fence has been placed here,
    // after all possible DMA actions have completed
    compiler_fence(SeqCst);
}

/// Stop an unfinished UART read transaction and flush FIFO to DMA buffer
fn uarte_cancel_read(uarte: &UARTE0) {
    uarte.events_rxto.write(|w| w);

    // Stop reception
    uarte.tasks_stoprx.write(|w| unsafe { w.bits(1) });

    // Wait for the reception to have stopped
    while uarte.events_rxto.read().bits() == 0 {}
    // for _ in 0..100_000 {
    //     if uarte.events_rxto.read().bits() != 0 {
    //         break;
    //     }
    // }

    // Reset the event flag
    uarte.events_rxto.write(|w| w);

    // Ask UART to flush FIFO to DMA buffer
    uarte.tasks_flushrx.write(|w| unsafe { w.bits(1) });

    // Wait for the flush to complete.
    while uarte.events_endrx.read().bits() == 0 {}
    // for _ in 0..100_000 {
    //     if uarte.events_endrx.read().bits() != 0 {
    //         break;
    //     }
    // }

    // The event flag itself is later reset by `finalize_read`.
}


fn uarte_setup<T: UarteInstance>(uarte: &T, mut pins: Pins, parity: Parity, baudrate: Baudrate) {
    // Select pins
    uarte.psel.rxd.write(|w| {
        let w = unsafe { w.pin().bits(pins.rxd.pin) };
        #[cfg(feature = "52840")]
        let w = w.port().bit(pins.rxd.port);
        w.connect().connected()
    });
    pins.txd.set_high().unwrap();
    uarte.psel.txd.write(|w| {
        let w = unsafe { w.pin().bits(pins.txd.pin) };
        #[cfg(feature = "52840")]
        let w = w.port().bit(pins.txd.port);
        w.connect().connected()
    });

    // Optional pins
    uarte.psel.cts.write(|w| {
        if let Some(ref pin) = pins.cts {
            let w = unsafe { w.pin().bits(pin.pin) };
            #[cfg(feature = "52840")]
            let w = w.port().bit(pin.port);
            w.connect().connected()
        } else {
            w.connect().disconnected()
        }
    });

    uarte.psel.rts.write(|w| {
        if let Some(ref pin) = pins.rts {
            let w = unsafe { w.pin().bits(pin.pin) };
            #[cfg(feature = "52840")]
            let w = w.port().bit(pin.port);
            w.connect().connected()
        } else {
            w.connect().disconnected()
        }
    });

    // Enable UARTE instance
    uarte.enable.write(|w| w.enable().enabled());

    // Configure
    let hardware_flow_control = pins.rts.is_some() && pins.cts.is_some();
    uarte
        .config
        .write(|w| w.hwfc().bit(hardware_flow_control).parity().variant(parity));

    // Configure frequency
    uarte.baudrate.write(|w| w.baudrate().variant(baudrate));

    uarte.intenclr.write(|w| unsafe { w.bits(0xFFFFFFFF) });

    uarte.intenset.write(|w| {
        w.endrx().set_bit();
        w.endtx().set_bit();
        w.error().set_bit();
        w
    });

}
