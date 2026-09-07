//! Single owner for the ESP32 Wi-Fi modem and reconnect state.

use anyhow::{anyhow, bail, Context, Result};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    wifi::{
        BlockingWifi, ClientConfiguration, Configuration, EspWifi, WifiDeviceId,
    },
};
use firmware_app_api::{NetworkHandle, NetworkSnapshot, NetworkState};
use log::{info, warn};

#[derive(Debug, Clone)]
pub struct WifiConfig {
    pub ssid: String,
    pub password: String,
}

pub struct WifiManager {
    wifi: BlockingWifi<EspWifi<'static>>,
    network: NetworkHandle,
}

impl WifiManager {
    pub fn new(
        modem: Modem<'static>,
        system_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        config: WifiConfig,
    ) -> Result<Self> {
        if config.ssid.is_empty() || config.ssid == "CHANGE_ME" {
            bail!("Set WIFI_SSID before building the firmware runtime");
        }
        if config.password == "CHANGE_ME" {
            bail!("Set WIFI_PASS before building the firmware runtime");
        }
        let mut wifi = BlockingWifi::wrap(
            EspWifi::new(modem, system_loop.clone(), Some(nvs))?,
            system_loop,
        )?;
        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: config
                .ssid
                .try_into()
                .map_err(|_| anyhow!("SSID is too long"))?,
            password: config
                .password
                .try_into()
                .map_err(|_| anyhow!("Wi-Fi password is too long"))?,
            ..Default::default()
        }))?;
        Ok(Self {
            wifi,
            network: NetworkHandle::new(),
        })
    }

    pub fn network_handle(&self) -> NetworkHandle {
        self.network.clone()
    }

    pub fn station_mac(&self) -> Result<[u8; 6]> {
        self.wifi
            .wifi()
            .get_mac(WifiDeviceId::Sta)
            .context("could not read station MAC address")
    }

    pub fn ensure_connected(&mut self) -> Result<String> {
        if self.wifi.is_connected().unwrap_or(false) && self.wifi.is_up().unwrap_or(false) {
            return self.publish_connected();
        }

        self.network.publish(NetworkSnapshot {
            state: NetworkState::Connecting,
            ip_address: None,
            last_error: None,
        });
        let connection = self.connect_once();
        match connection {
            Ok(ip_address) => Ok(ip_address),
            Err(error) => {
                let message = format!("{error:#}");
                self.network.publish(NetworkSnapshot {
                    state: NetworkState::Disconnected,
                    ip_address: None,
                    last_error: Some(message.clone()),
                });
                warn!("Wi-Fi unavailable: {message}");
                Err(error)
            }
        }
    }

    fn connect_once(&mut self) -> Result<String> {
        if !self.wifi.is_started()? {
            self.wifi.start().context("could not start Wi-Fi")?;
        }
        if !self.wifi.is_connected()? {
            self.wifi.connect().context("could not connect to access point")?;
        }
        self.wifi
            .wait_netif_up()
            .context("Wi-Fi connected but network interface did not come up")?;
        self.publish_connected()
    }

    fn publish_connected(&self) -> Result<String> {
        let ip_address = self
            .wifi
            .wifi()
            .sta_netif()
            .get_ip_info()
            .context("could not read station IP address")?
            .ip
            .to_string();
        self.network.publish(NetworkSnapshot {
            state: NetworkState::Connected,
            ip_address: Some(ip_address.clone()),
            last_error: None,
        });
        info!("Wi-Fi connected at {ip_address}");
        Ok(ip_address)
    }
}
