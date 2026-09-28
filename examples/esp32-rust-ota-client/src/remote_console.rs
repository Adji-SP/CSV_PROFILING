//! Rust application logs over MQTT/TLS. USB logging remains active.
//! C/ROM output and println! are not intercepted.
use esp_idf_svc::{
    log::EspIdfLogger,
    mqtt::client::{EspMqttClient, EventPayload, MqttClientConfiguration, QoS},
    tls::X509,
};
use log::{Log, Metadata, Record};
use std::{
    fmt::Write,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc,
    },
    time::Duration,
};

struct Entry {
    level: &'static str,
    target: String,
    message: String,
}
struct Logger {
    serial: EspIdfLogger<()>,
    sender: SyncSender<Entry>,
    dropped: Arc<AtomicU32>,
}
struct Text(String);
impl Write for Text {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let remaining = 1024usize.saturating_sub(self.0.len());
        let mut end = remaining.min(s.len());
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push_str(&s[..end]);
        Ok(())
    }
}
impl Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        self.serial.log(r);
        let mut message = Text(String::with_capacity(1024));
        let _ = write!(message, "{}", r.args());
        let entry = Entry {
            level: r.level().as_str(),
            target: r.target().chars().take(64).collect(),
            message: message.0,
        };
        if self.sender.try_send(entry).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn flush(&self) {
        self.serial.flush();
    }
}

fn certificate(value: Option<&str>) -> Option<X509<'static>> {
    value.filter(|v| !v.is_empty()).map(|v| {
        let pem = Box::leak(format!("{v}\0").into_bytes().into_boxed_slice());
        X509::pem_until_nul(pem)
    })
}

pub fn initialize() -> anyhow::Result<()> {
    let Some(url) = option_env!("DEVICE_MQTT_URL").filter(|s| !s.is_empty()) else {
        esp_idf_svc::log::EspLogger::initialize_default();
        return Ok(());
    };
    anyhow::ensure!(
        url.starts_with("mqtts://"),
        "Remote console requires MQTT over TLS"
    );
    anyhow::ensure!(
        option_env!("DEVICE_MQTT_CERT_PEM").is_some()
            == option_env!("DEVICE_MQTT_KEY_PEM").is_some(),
        "Provide MQTT client certificate and private key together"
    );
    // Provision a distinct identity and broker ACL for each board.
    let username = option_env!("DEVICE_MQTT_USERNAME")
        .ok_or_else(|| anyhow::anyhow!("DEVICE_MQTT_USERNAME required"))?;
    let id = option_env!("DEVICE_ID")
        .ok_or_else(|| anyhow::anyhow!("Set DEVICE_ID when enabling remote console"))?;
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
        "Invalid DEVICE_ID"
    );
    let config = MqttClientConfiguration {
        client_id: Some(id),
        username: Some(username),
        password: option_env!("DEVICE_MQTT_PASSWORD"),
        server_certificate: certificate(option_env!("DEVICE_MQTT_CA_PEM")),
        client_certificate: certificate(option_env!("DEVICE_MQTT_CERT_PEM")),
        private_key: certificate(option_env!("DEVICE_MQTT_KEY_PEM")),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        network_timeout: Duration::from_secs(5),
        outbox_limit: Some(8192),
        ..Default::default()
    };
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_connected = connected.clone();
    let mut mqtt = EspMqttClient::new_cb(url, &config, move |event| match event.payload() {
        EventPayload::Connected(_) => event_connected.store(true, Ordering::Relaxed),
        EventPayload::Disconnected => event_connected.store(false, Ordering::Relaxed),
        _ => {}
    })?;
    let (sender, receiver) = sync_channel::<Entry>(64);
    let dropped = Arc::new(AtomicU32::new(0));
    let count = dropped.clone();
    // Heap allocation is performed once; logger intentionally lives for device lifetime.
    let logger = Box::leak(Box::new(Logger {
        serial: EspIdfLogger::new(()),
        sender,
        dropped,
    }));
    log::set_logger(logger).map_err(|_| anyhow::anyhow!("Logger already installed"))?;
    log::set_max_level(log::LevelFilter::Info);
    let telemetry = firmware_app_api::tinyml::install();
    std::thread::Builder::new()
        .name("remote-console".into())
        .stack_size(8192)
        .spawn(move || {
            let topic = format!("devices/{id}/logs");
            let mut pending: Option<firmware_app_api::tinyml::Packet> = None;
            loop {
                if !connected.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                // Telemetry and logs share this MQTT client. Never block the application thread.
                for _ in 0..8 {
                    if pending.is_none() {
                        pending = telemetry.as_ref().and_then(|rx| rx.try_recv().ok());
                    }
                    let Some(packet) = pending.as_ref() else {
                        break;
                    };
                    if mqtt
                        .publish(&packet.topic, QoS::AtLeastOnce, false, &packet.payload)
                        .is_err()
                    {
                        break;
                    }
                    pending = None;
                }
                let Ok(entry) = receiver.recv_timeout(Duration::from_millis(50)) else {
                    continue;
                };
                let lost = count.swap(0, Ordering::Relaxed);
                let payload = serde_json::json!({
                    "device_id":id, "level":entry.level, "target":entry.target,
                    "message":entry.message, "dropped":lost
                })
                .to_string();
                if mqtt
                    .publish(&topic, QoS::AtMostOnce, false, payload.as_bytes())
                    .is_err()
                {
                    count.fetch_add(lost.saturating_add(1), Ordering::Relaxed);
                }
            }
        })?;
    Ok(())
}
