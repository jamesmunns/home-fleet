use crate::Error;
use bbqueue::{ArrayLength, BBBuffer};

use crate::hal::pac::UARTE0;
use crate::hal::ppi::{ConfigurablePpi, Ppi};
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

pub struct UarteBuffer<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub txd_buf: BBBuffer<OutgoingLen>,
    pub rxd_buf: BBBuffer<IncomingLen>,
    pub timeout_flag: AtomicBool,
}

pub struct UarteParts<OutgoingLen, IncomingLen, Timer, Channel>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
    Timer: TimerInstance,
    Channel: Ppi + ConfigurablePpi,
{
    pub app: UarteApp<OutgoingLen, IncomingLen>,
    pub timer: UarteTimer<Timer>,
    pub irq: UarteIrq<OutgoingLen, IncomingLen, Channel>,
}

impl<OutgoingLen, IncomingLen> UarteBuffer<OutgoingLen, IncomingLen>
where
    OutgoingLen: ArrayLength<u8>,
    IncomingLen: ArrayLength<u8>,
{
    pub fn try_split<Timer: TimerInstance, Channel: Ppi + ConfigurablePpi>(
        &'static self,
        pins: Pins,
        parity: Parity,
        baudrate: Baudrate,
        timer: Timer,
        mut ppi_ch: Channel,
        uarte: UARTE0,
        rx_block_size: usize,
        idle_us: u32,
    ) -> Result<UarteParts<OutgoingLen, IncomingLen, Timer, Channel>, Error> {
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

        let mut utim = UarteTimer {
            timer,
            timeout_flag: &self.timeout_flag,
        };

        ppi_ch.set_task_endpoint(unsafe { &(&*hw_timer).tasks_clear });
        ppi_ch.set_event_endpoint(&uarte.events_rxdrdy);

        let mut uirq = UarteIrq {
            incoming_prod: rxd_prod,
            outgoing_cons: txd_cons,
            timeout_flag: &self.timeout_flag,
            rx_grant: None,
            tx_grant: None,
            uarte,
            block_size: rx_block_size,
            ppi_ch,
        };

        utim.init(idle_us);
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
