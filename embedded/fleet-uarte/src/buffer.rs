use crate::Error;
use bbqueue::{ArrayLength, BBBuffer};

use crate::hal::pac::UARTE0;
use crate::hal::timer::Instance as TimerInstance;
use crate::hal::uarte::{Baudrate, Parity, Pins};
use crate::{
    app::UarteApp,
    irq::{UarteIrq, UarteTimer},
};
use core::sync::atomic::AtomicBool;

use crate::hal::pac::{Interrupt, TIMER0, TIMER1, TIMER2};
#[cfg(any(feature = "52832", feature = "52840"))]
use crate::hal::pac::{TIMER3, TIMER4};

use crate::hal::pac::ppi::CH;

pub struct UarteBuffer<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub txd_buf: BBBuffer<OutgoingLen>,
    pub rxd_buf: BBBuffer<IncomingLen>,
    pub timeout_flag: AtomicBool,
}

pub struct UarteParts<OutgoingLen, IncomingLen, Timer>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    Timer: TimerInstance,
{
    pub app: UarteApp<OutgoingLen, IncomingLen>,
    pub timer: UarteTimer<Timer>,
    pub irq: UarteIrq<OutgoingLen, IncomingLen>,
}

impl<OutgoingLen, IncomingLen> UarteBuffer<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub fn try_split<Timer: TimerInstance>(
        &'static self,
        pins: Pins,
        parity: Parity,
        baudrate: Baudrate,
        timer: Timer,
        ppi_ch: &CH,
        uarte: UARTE0,
        rx_block_size: usize
    ) -> Result<UarteParts<OutgoingLen, IncomingLen, Timer>, Error> {
        let (txd_prod, txd_cons) = self.txd_buf.try_split().map_err(|_| Error::Todo)?;
        let (rxd_prod, rxd_cons) = self.rxd_buf.try_split().map_err(|_| Error::Todo)?;

        // hmmm
        let hw_timer = match Timer::INTERRUPT {
            Interrupt::TIMER0 => TIMER0::ptr(),
            Interrupt::TIMER1 => TIMER1::ptr(),
            Interrupt::TIMER2 => TIMER2::ptr(),

            #[cfg(any(feature = "52832", feature = "52840"))]
            Interrupt::TIMER3 => TIMER3::ptr().cast(), // double yolo

            #[cfg(any(feature = "52832", feature = "52840"))]
            Interrupt::TIMER4 => TIMER4::ptr().cast(), // double yolo

            _ => unreachable!(),
        };

        // YOLO
        let clear_task_addr = unsafe { &(&*hw_timer).tasks_clear as *const _ as u32 };
        let rxdrdy_evt_addr = &uarte.events_rxdrdy as *const _ as u32;

        ppi_ch.eep.write(|w| unsafe { w.bits(rxdrdy_evt_addr) });
        ppi_ch.tep.write(|w| unsafe { w.bits(clear_task_addr) });

        let mut utim = UarteTimer {
            timer,
            timeout_flag: &self.timeout_flag,
        };

        let mut uirq = UarteIrq {
            incoming_prod: rxd_prod,
            outgoing_cons: txd_cons,
            timeout_flag: &self.timeout_flag,
            rx_grant: None,
            tx_grant: None,
            uarte,
            block_size: rx_block_size,
        };

        utim.init(1_000_000);
        uirq.init(pins, parity, baudrate);

        // ...
        Ok(UarteParts {
            app: UarteApp {
                outgoing_prod: txd_prod,
                incoming_cons: rxd_cons,
            },
            irq: uirq,
            timer: utim,
        })
    }
}
