//! Single owner for the ESP32 Wi-Fi modem and reconnect state.

use anyhow::{anyhow, bail, Context, Result};
use esp_idf_svc::{
    eventloop::{EspSystemEventLoop, EspSystemSubscription},
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    wifi::{
        BlockingWifi, ClientConfiguration, Configuration, EspWifi, WifiDeviceId, WifiEvent,
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
    ssid: String,
    _diagnostic_subscription: EspSystemSubscription<'static>,
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
        let diagnostic_subscription = system_loop.subscribe::<WifiEvent, _>(|event| {
            if let WifiEvent::StaDisconnected(details) = event {
                let reason = details.reason();
                warn!(
                    "Wi-Fi disconnected: reason {reason} ({})",
                    disconnect_reason_description(reason)
                );
            }
        })?;
        let ssid = config.ssid;
        let mut wifi = BlockingWifi::wrap(
            EspWifi::new(modem, system_loop.clone(), Some(nvs))?,
            system_loop,
        )?;
        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid
                .as_str()
                .try_into()
                .map_err(|_| anyhow!("SSID is too long"))?,
            password: config
                .password
                .as_str()
                .try_into()
                .map_err(|_| anyhow!("Wi-Fi password is too long"))?,
            ..Default::default()
        }))?;
        Ok(Self {
            wifi,
            network: NetworkHandle::new(),
            ssid,
            _diagnostic_subscription: diagnostic_subscription,
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
            self.log_target_scan()?;
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

    fn log_target_scan(&mut self) -> Result<()> {
        let access_points = self
            .wifi
            .scan()
            .context("could not scan for nearby Wi-Fi access points")?;
        let target = access_points
            .iter()
            .filter(|access_point| access_point.ssid.as_str() == self.ssid)
            .max_by_key(|access_point| access_point.signal_strength);

        match target {
            Some(access_point) => {
                info!(
                    "Configured Wi-Fi found: SSID='{}', channel={}, RSSI={} dBm, authentication={:?}",
                    self.ssid,
                    access_point.channel,
                    access_point.signal_strength,
                    access_point.auth_method
                );
                Ok(())
            }
            None => bail!(
                "configured SSID '{}' was not visible in the 2.4 GHz scan ({} access points found)",
                self.ssid,
                access_points.len()
            ),
        }
    }
}

fn disconnect_reason_description(reason: u16) -> &'static str {
    match reason {
        2 => "authentication expired",
        4 => "association expired",
        15 => "WPA four-way handshake timed out; check the password",
        200 => "access-point beacon timed out",
        201 => "configured access point was not found",
        202 => "authentication failed; check the password and security mode",
        203 => "association failed or the access point rejected the device",
        204 => "WPA handshake timed out; check the password",
        205 => "connection failed",
        210 => "no access point with compatible security was found",
        211 => "access point did not meet the configured authentication threshold",
        212 => "access point signal was below the configured threshold",
        _ => "see ESP-IDF Wi-Fi reason codes",
    }
}

#[cfg(test)]
mod tests {
    use super::disconnect_reason_description;

    #[test]
    fn explains_common_authentication_failures() {
        assert!(disconnect_reason_description(202).contains("password"));
        assert!(disconnect_reason_description(204).contains("password"));
        assert!(disconnect_reason_description(201).contains("not found"));
    }
}
