//! Example user configuration for the demo.
//!
//! Put your actual configuration in `config.rs` (which should automatically start out as a copy of
//! this `config.example.rs` file).

config! {
    // The baudrate to configure the UART with.
    // Any variant of `nrf52810_hal::uarte::Baudrate` is accepted.
    baudrate = BAUD230400;

    // UART TX and RX pins.
    // NOTE: These pins are for the dwm1001
    tx_pin = p0_05;
    rx_pin = p0_11;
}
