# Hardware UARTE

Wrapper structure that takes the following:

* Timer
* UARTE
* Incoming/Outgoing BBQueue

Basic gist:

* Set a timeout to one of the CC registers, something like 5 * (1 / (baudrate / 10))
* Set timer to count up
* Set a shortcut between UARTE::RXDRDY -> TIMER::CLEAR
* Enable timer interrupt, in handler:
    * Flush RX
    * Commit BBQueue
    * Get new grant
    * Start RX
    * Clear/restart timer
* Enable ENDRX interrupt, in handler:
    * Flush RX
    * Commit BBQueue
    * Get new grant
    * Start RX
    * Clear/restart timer
* Enable ENDTX interrupt, in handler:
    * Release BBQueue

* App
    * send_grant
        * put stuff in buffer
    * commit
        * commit
        * trigger uarte interrupt (note: we need the interrupt)
    * rx_grant
        * pull stuff from buffer
    * release
        * maybe fire interrupt just in case buffer was full?

* Irq
    * uarte_interrupt
        * Check for tx/rx complete
    * timer_interrupt
        * cycle rx buffer
