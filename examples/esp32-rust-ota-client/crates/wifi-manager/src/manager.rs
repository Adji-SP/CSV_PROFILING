use crate::{
    portal::{Command, Portal, PortalState, SharedState},
    settings::{
        should_open_portal, WifiConfig, ATTEMPT_SECONDS, CLOSE_SECONDS, PORTAL_IP, RETRY_SECONDS,
    },
    storage::SettingsStore,
};
use anyhow::{anyhow, bail, Result};
use embedded_svc::ipv4;
use esp_idf_svc::{
    eventloop::{EspSystemEventLoop, EspSystemSubscription},
    hal::modem::Modem,
    netif::{EspNetif, NetifConfiguration, NetifStack},
    nvs::EspDefaultNvsPartition,
    wifi::{
        AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration, EspWifi,
        WifiDeviceId, WifiDriver, WifiEvent,
    },
};
use firmware_app_api::{NetworkHandle, NetworkSnapshot, NetworkState};
use log::{info, warn};
use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub struct WifiManager {
    network: NetworkHandle,
    mac: [u8; 6],
    worker: Option<Worker<()>>,
}

impl WifiManager {
    pub fn new(
        modem: Modem<'static>,
        system_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        bootstrap: Option<WifiConfig>,
    ) -> Result<Self> {
        let (store, stored) = SettingsStore::open(nvs.clone())?;
        let persisted = stored.is_some();
        let saved = stored.or(bootstrap);
        let network = NetworkHandle::new();
        let subscription = system_loop.subscribe::<WifiEvent, _>(|event| {
            if let WifiEvent::StaDisconnected(details) = event {
                warn!(
                    "Wi-Fi disconnected (reason {}). Retrying; saved settings are retained.",
                    details.reason()
                );
            }
        })?;
        // esp-idf-svc defaults to 192.168.71.1 and public DNS. Configure both
        // explicitly so the offline portal and DHCP-advertised DNS agree.
        let gateway = PORTAL_IP.parse()?;
        let ap_netif = EspNetif::new_with_conf(&NetifConfiguration {
            ip_configuration: Some(ipv4::Configuration::Router(ipv4::RouterConfiguration {
                subnet: ipv4::Subnet {
                    gateway,
                    mask: ipv4::Mask(24),
                },
                dhcp_enabled: true,
                dns: Some(gateway),
                secondary_dns: None,
            })),
            ..NetifConfiguration::wifi_default_router()
        })?;
        let mut wifi = EspWifi::wrap_all(
            WifiDriver::new(modem, system_loop, Some(nvs))?,
            EspNetif::new(NetifStack::Sta)?,
            ap_netif,
        )?;
        // Only our two-slot settings store commits credentials, after successful DHCP.
        esp_idf_svc::sys::esp!(unsafe {
            esp_idf_svc::sys::esp_wifi_set_storage(
                esp_idf_svc::sys::wifi_storage_t_WIFI_STORAGE_RAM,
            )
        })?;
        wifi.set_configuration(&Configuration::Client(client(saved.as_ref())?))?;
        wifi.start()?;
        let mac = wifi.get_mac(WifiDeviceId::Sta)?;
        let setup_password = store.setup_password()?;
        let ap_name = format!("ESP32-Setup-{:02X}{:02X}{:02X}", mac[3], mac[4], mac[5]);
        let state = Arc::new(Mutex::new(PortalState::new(&ap_name, saved.as_ref())));
        let (tx, rx) = mpsc::sync_channel(2);
        Ok(Self {
            network: network.clone(),
            mac,
            worker: Some(Worker {
                wifi,
                store,
                saved,
                persisted,
                active: None,
                candidate: false,
                attempt_started: None,
                disconnected_since: Instant::now(),
                next_retry: Instant::now(),
                close_at: None,
                network,
                state,
                tx,
                rx,
                portal: (),
                ap_name,
                setup_password,
                _subscription: subscription,
            }),
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            // Print once on USB at every boot so owners can record the recovery key
            // even if bootstrap Wi-Fi connects and the hotspot never needs to open.
            println!("\nRecovery hotspot: {}\nRecovery Wi-Fi password: {}\nSetup URL when hotspot is active: http://{PORTAL_IP}\n",
                worker.ap_name, worker.setup_password);
            thread::Builder::new()
                .name("wifi-supervisor".into())
                .stack_size(12 * 1024)
                .spawn(move || {
                    // HTTP handles are !Send: create and destroy them on this task.
                    let mut worker = worker.with_portal();
                    loop {
                        if let Err(error) = worker.tick() {
                            warn!("Wi-Fi supervisor: {error:#}");
                            worker.fail_attempt("Wi-Fi operation failed. Please retry.");
                            thread::sleep(Duration::from_secs(2));
                        }
                        thread::sleep(Duration::from_millis(200));
                    }
                })?;
        }
        Ok(())
    }

    pub fn station_mac(&self) -> Result<[u8; 6]> {
        Ok(self.mac)
    }
    pub fn network_handle(&self) -> NetworkHandle {
        self.network.clone()
    }
    pub fn ensure_connected(&mut self) -> Result<String> {
        let snapshot = self.network.snapshot();
        if snapshot.state == NetworkState::Connected {
            return snapshot
                .ip_address
                .ok_or_else(|| anyhow!("Waiting for DHCP"));
        }
        bail!("Wi-Fi supervisor is reconnecting or awaiting setup")
    }
}

struct Worker<P> {
    wifi: EspWifi<'static>,
    store: SettingsStore,
    saved: Option<WifiConfig>,
    persisted: bool,
    active: Option<WifiConfig>,
    candidate: bool,
    attempt_started: Option<Instant>,
    disconnected_since: Instant,
    next_retry: Instant,
    close_at: Option<Instant>,
    network: NetworkHandle,
    state: SharedState,
    tx: SyncSender<Command>,
    rx: Receiver<Command>,
    portal: P,
    ap_name: String,
    setup_password: String,
    _subscription: EspSystemSubscription<'static>,
}

impl Worker<()> {
    fn with_portal(self) -> Worker<Option<Portal>> {
        Worker {
            wifi: self.wifi,
            store: self.store,
            saved: self.saved,
            persisted: self.persisted,
            active: self.active,
            candidate: self.candidate,
            attempt_started: self.attempt_started,
            disconnected_since: self.disconnected_since,
            next_retry: self.next_retry,
            close_at: self.close_at,
            network: self.network,
            state: self.state,
            tx: self.tx,
            rx: self.rx,
            portal: None,
            ap_name: self.ap_name,
            setup_password: self.setup_password,
            _subscription: self._subscription,
        }
    }
}

impl Worker<Option<Portal>> {
    fn tick(&mut self) -> Result<()> {
        if let Ok(command) = self.rx.try_recv() {
            match command {
                Command::Connect(config) => self.attempt(config, true)?,
                Command::Retry => {
                    if let Some(config) = self.saved.clone() {
                        self.attempt(config, false)?;
                    } else {
                        self.fail_attempt("No saved network yet. Choose a network first.");
                    }
                }
                Command::Scan => self.scan()?,
            }
        }
        // A late driver event after a rejected attempt must not be treated as success.
        let expected_connection = self.active.is_some() || self.network.is_connected();
        let connected =
            expected_connection && self.wifi.is_connected()? && self.wifi.sta_netif().is_up()?;
        if connected {
            let ip = self.wifi.sta_netif().get_ip_info()?.ip.to_string();
            if ip == "0.0.0.0" {
                return Ok(());
            }
            if let Some(config) = self.active.take() {
                if !self.persisted || self.saved.as_ref() != Some(&config) {
                    if self.store.save(&config).is_err() {
                        self.fail_attempt("Connected, but settings could not be saved. Previous settings are unchanged; retry.");
                        return Ok(());
                    }
                    self.saved = Some(config);
                    self.persisted = true;
                }
            }
            if self.network.snapshot().state != NetworkState::Connected {
                info!("Wi-Fi connected at {ip}");
            }
            self.attempt_started = None;
            self.candidate = false;
            self.network.publish(NetworkSnapshot {
                state: NetworkState::Connected,
                ip_address: Some(ip.clone()),
                last_error: None,
            });
            if self.portal.is_some() && self.close_at.is_none() {
                self.close_at = Some(Instant::now() + Duration::from_secs(CLOSE_SECONDS));
            }
            {
                let mut state = self.state.lock().unwrap();
                state.phase = "connected".into();
                state.message = "Wi-Fi connected. Settings are saved on this board.".into();
                state.ip_address = Some(ip);
                state.saved_ssid = self.saved.as_ref().map(|c| c.ssid.clone());
                state.busy = false;
                state.close_in = self
                    .close_at
                    .map(|time| time.saturating_duration_since(Instant::now()).as_secs());
            }
            if self.close_at.is_some_and(|time| Instant::now() >= time) {
                self.close_portal()?;
            }
            return Ok(());
        }
        if self.network.snapshot().state == NetworkState::Connected {
            self.disconnected_since = Instant::now();
            self.next_retry = Instant::now();
        }
        self.close_at = None;
        self.network.publish(NetworkSnapshot {
            state: if self.attempt_started.is_some() {
                NetworkState::Connecting
            } else {
                NetworkState::Disconnected
            },
            ip_address: None,
            last_error: None,
        });
        {
            let mut state = self.state.lock().unwrap();
            state.ip_address = None;
            state.close_in = None;
            if state.phase == "connected" {
                state.phase = "idle".into();
            }
        }
        if self.portal.is_none()
            && should_open_portal(
                self.saved.is_some(),
                self.disconnected_since.elapsed().as_secs(),
            )
        {
            self.open_portal()?;
        }
        if self
            .attempt_started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(ATTEMPT_SECONDS))
        {
            self.fail_attempt("Could not connect. Check the password, signal and 2.4 GHz network. Previous settings are unchanged.");
        }
        if self.attempt_started.is_none() && Instant::now() >= self.next_retry {
            if let Some(config) = self.saved.clone() {
                self.attempt(config, false)?;
            }
        }
        Ok(())
    }

    fn configuration(&self, station: Option<&WifiConfig>, ap: bool) -> Result<Configuration> {
        let station = client(station)?;
        Ok(if ap {
            Configuration::Mixed(
                station,
                AccessPointConfiguration {
                    ssid: self
                        .ap_name
                        .as_str()
                        .try_into()
                        .map_err(|_| anyhow!("Setup name too long"))?,
                    password: self
                        .setup_password
                        .as_str()
                        .try_into()
                        .map_err(|_| anyhow!("Setup password too long"))?,
                    auth_method: AuthMethod::WPA2Personal,
                    channel: 1,
                    max_connections: 4,
                    ..Default::default()
                },
            )
        } else {
            Configuration::Client(station)
        })
    }

    fn attempt(&mut self, config: WifiConfig, candidate: bool) -> Result<()> {
        self.disconnect_station()?;
        self.wifi
            .set_configuration(&self.configuration(Some(&config), self.portal.is_some())?)?;
        self.network.publish(NetworkSnapshot {
            state: NetworkState::Connecting,
            ip_address: None,
            last_error: None,
        });
        self.close_at = None;
        {
            let mut state = self.state.lock().unwrap();
            // Keep a failed submission visible while background retries run.
            if candidate || state.phase != "failed" {
                state.target_ssid = Some(config.ssid.clone());
                state.phase = "connecting".into();
                state.message = "Connecting to Wi-Fi and waiting for an IP address...".into();
            }
            // Background retries must not lock out a new setup submission.
            state.busy = candidate;
        }
        self.active = Some(config);
        self.candidate = candidate;
        self.attempt_started = Some(Instant::now());
        self.wifi.connect()?;
        Ok(())
    }

    fn fail_attempt(&mut self, message: &str) {
        let _ = self.wifi.disconnect();
        self.network.publish(NetworkSnapshot {
            state: NetworkState::Disconnected,
            ip_address: None,
            last_error: Some(message.into()),
        });
        self.active = None;
        self.attempt_started = None;
        self.candidate = false;
        self.close_at = None;
        self.next_retry = Instant::now() + Duration::from_secs(RETRY_SECONDS);
        let mut state = self.state.lock().unwrap();
        state.phase = "failed".into();
        state.message = message.into();
        state.busy = false;
        state.scanning = false;
        state.ip_address = None;
        state.close_in = None;
    }

    fn open_portal(&mut self) -> Result<()> {
        self.disconnect_station()?;
        self.active = None;
        self.attempt_started = None;
        self.wifi
            .set_configuration(&self.configuration(self.saved.as_ref(), true)?)?;
        for _ in 0..20 {
            if self.wifi.ap_netif().is_up()? {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.portal = Some(Portal::start(self.state.clone(), self.tx.clone())?);
        // Serial only: never use log! here, which is forwarded to MQTT.
        println!(
            "\nWi-Fi setup network: {}\nWi-Fi setup password: {}\nOpen http://{PORTAL_IP}\n",
            self.ap_name, self.setup_password
        );
        info!("Wi-Fi setup portal opened; connection instructions are on USB serial.");
        self.scan()?;
        self.next_retry = Instant::now() + Duration::from_secs(RETRY_SECONDS);
        Ok(())
    }

    fn disconnect_station(&mut self) -> Result<()> {
        let _ = self.wifi.disconnect();
        // Wait for the driver + DHCP disconnect events before changing credentials.
        // Otherwise cached "connected" state could validate an untested password.
        for _ in 0..40 {
            if !self.wifi.is_connected()? && !self.wifi.sta_netif().is_up()? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        bail!("Wi-Fi disconnect is still pending; retrying without changing saved settings")
    }

    fn close_portal(&mut self) -> Result<()> {
        self.portal.take();
        self.close_at = None;
        self.wifi
            .set_configuration(&self.configuration(self.saved.as_ref(), false)?)?;
        info!("Wi-Fi setup hotspot closed.");
        Ok(())
    }

    fn scan(&mut self) -> Result<()> {
        if self.attempt_started.is_some() {
            let mut state = self.state.lock().unwrap();
            state.scanning = false;
            state.scan_error = Some("A connection attempt is running. Try scanning again shortly, or enter a network name.".into());
            return Ok(());
        }
        let result = self.wifi.scan_n::<32>();
        let mut state = self.state.lock().unwrap();
        state.scanning = false;
        match result {
            Ok((points, _)) => {
                let mut networks: Vec<crate::portal::VisibleNetwork> = points
                    .iter()
                    .filter(|p| !p.ssid.is_empty())
                    .map(|p| crate::portal::VisibleNetwork {
                        ssid: p.ssid.to_string(),
                        rssi: p.signal_strength,
                        secured: p.auth_method != Some(AuthMethod::None),
                    })
                    .collect();
                networks.sort_by(|a, b| b.rssi.cmp(&a.rssi));
                let mut seen = std::collections::HashSet::new();
                networks.retain(|p| seen.insert(p.ssid.clone()));
                state.networks = networks;
                state.scan_error = None;
            }
            Err(_) => {
                state.scan_error =
                    Some("Scan unavailable. Retry or enter the Wi-Fi name manually.".into())
            }
        }
        Ok(())
    }
}

fn client(config: Option<&WifiConfig>) -> Result<ClientConfiguration> {
    let Some(config) = config else {
        return Ok(ClientConfiguration::default());
    };
    Ok(ClientConfiguration {
        ssid: config
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow!("SSID too long"))?,
        password: config
            .password
            .as_str()
            .try_into()
            .map_err(|_| anyhow!("Password too long"))?,
        auth_method: if config.password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    })
}
