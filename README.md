# James' Home Fleet

A fleet of devices running on my home network

* My PC, acting as a bridge to the "PAN" of my own devices
* A bunch of devices:
    * Kitchen Plant Light Controller
    * Living Room Plant Light Controller
    * Blackberry PDA
* The network is:
    * Lowest level is basically Enhanced ShockBurst
    * Then, ChaCha8Poly1305 for authenticated crypto
        * In the future, probably replace with AES128-gcm-siv
        * Today: max of 200us to encrypt/decrypt a 250 byte message

* Shockburst terms:
    * PRX - Primarily Receiving
        * Usually a PC
    * PTX - Primarily Transmitting
        * Usually a Microcontroller

## Plant Light

* Control Relays
    * Make sure relays don't toggle too often
    * Make sure relays go out if we lose comms to server
* Manage Comms
    * Periodically ask for incoming messages
    * Drain outgoing queue
* Running timer for comms + relays


# License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
