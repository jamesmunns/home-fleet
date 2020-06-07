pub mod demo;

#[cfg(feature = "prod")]
pub mod prod;

#[cfg(all(feature = "prod", not(feature = "demo")))]
pub use prod::PROD_KEY as KEY;

#[cfg(all(feature = "demo", not(feature = "prod")))]
pub use demo::DEMO_KEY as KEY;
