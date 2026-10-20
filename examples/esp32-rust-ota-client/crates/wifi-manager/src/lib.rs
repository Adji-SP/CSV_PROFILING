//! Persistent Wi-Fi configuration and an offline, AP-only setup portal.
pub mod settings;
pub use settings::WifiConfig;
#[cfg(target_os = "espidf")]
mod manager;
#[cfg(target_os = "espidf")]
mod portal;
#[cfg(target_os = "espidf")]
mod storage;
#[cfg(target_os = "espidf")]
pub use manager::WifiManager;
