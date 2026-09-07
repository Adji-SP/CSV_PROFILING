//! Shared data passed from the stable runtime to a function-based device application.

use std::sync::{Arc, RwLock};

use esp_idf_svc::hal::{
    adc::{ADC1, ADC2},
    can::CAN,
    gpio::Pins,
    i2c::{I2C0, I2C1},
    ledc::LEDC,
    modem::Modem,
    peripherals::Peripherals,
    spi::{SPI2, SPI3},
    uart::{UART1, UART2},
};

pub use esp_idf_svc::hal;

pub const API_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone)]
pub struct NetworkSnapshot {
    pub state: NetworkState,
    pub ip_address: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct NetworkHandle {
    inner: Arc<RwLock<NetworkSnapshot>>,
}

impl Default for NetworkHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(NetworkSnapshot {
                state: NetworkState::Disconnected,
                ip_address: None,
                last_error: None,
            })),
        }
    }

    pub fn snapshot(&self) -> NetworkSnapshot {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn is_connected(&self) -> bool {
        self.snapshot().state == NetworkState::Connected
    }

    #[doc(hidden)]
    pub fn publish(&self, snapshot: NetworkSnapshot) {
        *self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
    }
}

pub struct ApplicationPeripherals {
    pub pins: Option<Pins>,
    pub uart1: Option<UART1<'static>>,
    pub uart2: Option<UART2<'static>>,
    pub i2c0: Option<I2C0<'static>>,
    pub i2c1: Option<I2C1<'static>>,
    pub spi2: Option<SPI2<'static>>,
    pub spi3: Option<SPI3<'static>>,
    pub adc1: Option<ADC1<'static>>,
    pub adc2: Option<ADC2<'static>>,
    pub can: Option<CAN<'static>>,
    pub ledc: Option<LEDC>,
}

impl ApplicationPeripherals {
    pub fn split_esp32s3(peripherals: Peripherals) -> (Modem<'static>, Self) {
        let Peripherals {
            pins,
            uart1,
            uart2,
            i2c0,
            i2c1,
            spi2,
            spi3,
            adc1,
            adc2,
            can,
            ledc,
            modem,
            ..
        } = peripherals;
        (
            modem,
            Self {
                pins: Some(pins),
                uart1: Some(uart1),
                uart2: Some(uart2),
                i2c0: Some(i2c0),
                i2c1: Some(i2c1),
                spi2: Some(spi2),
                spi3: Some(spi3),
                adc1: Some(adc1),
                adc2: Some(adc2),
                can: Some(can),
                ledc: Some(ledc),
            },
        )
    }
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub chip: String,
    pub firmware_version: String,
}

pub struct AppContext {
    pub device: DeviceInfo,
    pub network: NetworkHandle,
    pub peripherals: ApplicationPeripherals,
}
