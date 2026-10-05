//! Read-only MQTT log ingestion and authenticated browser streaming.
use crate::{
    errors::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::Response,
};
use rumqttc::{
    AsyncClient, ConnectReturnCode, Event, Incoming, MqttOptions, QoS, SubscribeReasonCode,
    Transport,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    time::Duration,
};
use tokio::sync::{Mutex, broadcast};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub device_id: String,
    pub level: String,
    pub message: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub received_at: String,
}

pub struct Console {
    token: String,
    mqtt_configured: bool,
    mqtt_connected: AtomicBool,
    history: Mutex<HashMap<String, VecDeque<LogEntry>>>,
    events: broadcast::Sender<LogEntry>,
}

impl Console {
    #[cfg(test)]
    pub(crate) fn for_test(token: String) -> Arc<Self> {
        let (events, _) = broadcast::channel(512);
        Arc::new(Self {
            token,
            mqtt_configured: false,
            mqtt_connected: AtomicBool::new(false),
            history: Mutex::new(HashMap::new()),
            events,
        })
    }
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(512);
        Arc::new(Self {
            token: std::env::var("CONSOLE_VIEWER_TOKEN").unwrap_or_default(),
            mqtt_configured: std::env::var("MQTT_HOST").is_ok(),
            mqtt_connected: AtomicBool::new(false),
            history: Mutex::new(HashMap::new()),
            events,
        })
    }

    pub fn mqtt_configured(&self) -> bool {
        self.mqtt_configured
    }

    pub fn mqtt_connected(&self) -> bool {
        self.mqtt_connected.load(Ordering::Relaxed)
    }

    fn set_mqtt_connected(&self, connected: bool, tinyml: &crate::tinyml::TinyMl) {
        if self.mqtt_connected.swap(connected, Ordering::Relaxed) != connected {
            let _ = tinyml.events.send(serde_json::json!({
                "type": "tinyml.broker.status", "connected": connected
            }));
        }
    }

    pub async fn accept(&self, topic: &str, payload: &[u8]) {
        let Some(id) = topic
            .strip_prefix("devices/")
            .and_then(|s| s.strip_suffix("/logs"))
        else {
            return;
        };
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            || payload.len() > 4096
        {
            return;
        }
        let Ok(mut entry) = serde_json::from_slice::<LogEntry>(payload) else {
            return;
        };
        if entry.device_id != id
            || entry.message.len() > 2048
            || entry.target.len() > 128
            || !["ERROR", "WARN", "INFO", "DEBUG", "TRACE"].contains(&entry.level.as_str())
        {
            return;
        }
        entry.received_at = chrono::Utc::now().to_rfc3339();
        let mut history = self.history.lock().await;
        // Bound total memory as well as per-device memory.
        if history.len() >= 256 && !history.contains_key(id) {
            return;
        }
        let rows = history.entry(id.to_owned()).or_default();
        if rows.len() >= 200 {
            rows.pop_front();
        }
        rows.push_back(entry.clone());
        let _ = self.events.send(entry);
    }

    pub fn start_mqtt(
        self: &Arc<Self>,
        tinyml: Arc<crate::tinyml::TinyMl>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Ok(host) = std::env::var("MQTT_HOST") else {
            return Ok(());
        };
        if host.trim().is_empty() {
            return Err("MQTT_HOST must not be empty when configured".into());
        }
        if self.token.len() < 32 {
            return Err("CONSOLE_VIEWER_TOKEN must contain at least 32 characters".into());
        }
        let port: u16 = std::env::var("MQTT_PORT")
            .unwrap_or("8883".into())
            .parse()
            .map_err(|_| "MQTT_PORT must be a number between 1 and 65535")?;
        if port == 0 {
            return Err("MQTT_PORT must be a number between 1 and 65535".into());
        }
        let mut options = MqttOptions::new(
            std::env::var("MQTT_CLIENT_ID").unwrap_or("ota-console-backend".into()),
            host,
            port,
        );
        options.set_keep_alive(Duration::from_secs(30));
        options.set_max_packet_size(
            tinyml.settings.max_bytes + 1024,
            tinyml.settings.max_bytes + 1024,
        );
        options.set_manual_acks(true);
        options.set_clean_session(false);
        options.set_credentials(
            std::env::var("MQTT_USERNAME")
                .map_err(|_| "MQTT_USERNAME is required when MQTT_HOST is set")?,
            std::env::var("MQTT_PASSWORD").unwrap_or_default(),
        );
        let ca = std::env::var("MQTT_CA_FILE").ok();
        let cert = std::env::var("MQTT_CERT_FILE").ok();
        let key = std::env::var("MQTT_KEY_FILE").ok();
        match (ca, cert, key) {
            (Some(ca), Some(cert), Some(key)) => {
                options.set_transport(Transport::tls(
                    std::fs::read(ca).map_err(|e| format!("MQTT_CA_FILE cannot be read: {e}"))?,
                    Some((
                        std::fs::read(cert).map_err(|e| format!("MQTT_CERT_FILE cannot be read: {e}"))?,
                        std::fs::read(key).map_err(|e| format!("MQTT_KEY_FILE cannot be read: {e}"))?,
                    )),
                    None,
                ));
            }
            (Some(ca), None, None) => {
                options.set_transport(Transport::tls(
                    std::fs::read(ca).map_err(|e| format!("MQTT_CA_FILE cannot be read: {e}"))?,
                    None,
                    None,
                ));
            }
            (None, None, None) => {
                let roots = rustls::RootCertStore::from_iter(
                    webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
                );
                let tls = rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                options.set_transport(Transport::tls_with_config(tls.into()));
            }
            _ => return Err(
                "MQTT client certificate/key require MQTT_CA_FILE and must be provided together"
                    .into(),
            ),
        }
        let (client, mut events) = AsyncClient::new(options, 32);
        let (ingestion_sender, mut ingestion_receiver) =
            tokio::sync::mpsc::channel::<rumqttc::Publish>(64);
        let ingestion_client = client.clone();
        let ingestion_service = tinyml.clone();
        let mut worker_shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {_=worker_shutdown.changed()=>break,message=ingestion_receiver.recv()=>match message {Some(m)=>m,None=>break}};
                // QoS 1 acknowledgment happens after the atomic database commit.
                loop {
                    match ingestion_service
                        .ingest(&message.topic, &message.payload)
                        .await
                    {
                        Ok(_) => break,
                        Err(error) if error.status.is_server_error() => {
                            tracing::error!(
                                code = error.code,
                                "TinyML storage unavailable; retrying before MQTT acknowledgment"
                            );
                            tokio::select! {_=worker_shutdown.changed()=>return,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
                        }
                        Err(error) => {
                            tracing::warn!(code=error.code,reason=%error.message,"TinyML message rejected");
                            break;
                        }
                    }
                }
                if ingestion_client.ack(&message).await.is_err() {
                    break;
                }
            }
        });
        let console = self.clone();
        tokio::spawn(async move {
            let mut pending_subscriptions = 0usize;
            loop {
                let next = tokio::select! {
                    _=shutdown.changed()=>{console.set_mqtt_connected(false, &tinyml);let _=client.try_disconnect();break;},
                    next=events.poll()=>next,
                };
                match next {
                    Ok(Event::Incoming(Incoming::ConnAck(ack))) => {
                        console.set_mqtt_connected(false, &tinyml);
                        pending_subscriptions = 0;
                        if ack.code != ConnectReturnCode::Success {
                            tracing::warn!(reason=?ack.code, "MQTT broker rejected the backend connection");
                            events.clean();continue;
                        }
                        if client.try_subscribe("devices/+/logs", QoS::AtMostOnce).is_err() {
                            console.set_mqtt_connected(false, &tinyml);
                            events.clean();continue;
                        }
                        pending_subscriptions += 1;
                        if tinyml.settings.enabled
                            && client
                                .try_subscribe(
                                    format!("{}/#", tinyml.settings.prefix),
                                    QoS::AtLeastOnce,
                                )
                                .is_err()
                        {
                            console.set_mqtt_connected(false, &tinyml);
                            events.clean();continue;
                        }
                        pending_subscriptions += usize::from(tinyml.settings.enabled);
                    }
                    Ok(Event::Incoming(Incoming::SubAck(ack))) => {
                        if ack.return_codes.iter().any(|code| matches!(code, SubscribeReasonCode::Failure)) {
                            tracing::warn!("MQTT broker denied a backend subscription; check its ACL");
                            console.set_mqtt_connected(false, &tinyml);
                            pending_subscriptions = 0;
                            events.clean();continue;
                        }
                        if pending_subscriptions > 0 {
                            pending_subscriptions -= 1;
                            if pending_subscriptions == 0 {
                                console.set_mqtt_connected(true, &tinyml);
                            }
                        }
                    }
                    Ok(Event::Incoming(Incoming::Publish(p))) if !p.retain => {
                        if tinyml.settings.enabled
                            && p.topic.starts_with(&format!("{}/", tinyml.settings.prefix))
                        {
                            match ingestion_sender.try_send(p) {
                                Ok(())=>{},
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_))=>break,
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_))=>{
                                    // Never block the MQTT poller behind the ACK request queue.
                                    // Close without ACK; the persistent broker session redelivers.
                                    tracing::warn!("TinyML ingestion backpressure; reconnecting for QoS 1 redelivery");
                                    console.set_mqtt_connected(false, &tinyml);
                                    events.clean();
                                    tokio::select!{_=shutdown.changed()=>break,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
                                }
                            }
                        } else {
                            console.accept(&p.topic, &p.payload).await;
                            if client.try_ack(&p).is_err(){console.set_mqtt_connected(false, &tinyml);events.clean();}
                        }
                    }
                    Ok(Event::Incoming(Incoming::Publish(p))) => {
                        if client.try_ack(&p).is_err(){console.set_mqtt_connected(false, &tinyml);events.clean();}
                    }
                    Ok(_) => {}
                    Err(error) => {
                        console.set_mqtt_connected(false, &tinyml);
                        pending_subscriptions = 0;
                        tracing::warn!(%error, "Console MQTT connection lost; retrying");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
        Ok(())
    }
}

#[derive(Deserialize)]
struct Hello {
    token: String,
    device_id: String,
}

pub(crate) fn authorized(console: &Console, token: &str) -> bool {
    use sha2::{Digest, Sha256};
    if console.token.len() < 32 {
        return false;
    }
    let expected = Sha256::digest(console.token.as_bytes());
    let actual = Sha256::digest(token.as_bytes());
    expected
        .iter()
        .zip(actual.iter())
        .fold(0u8, |a, (x, y)| a | (x ^ y))
        == 0
}

pub async fn devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<axum::Json<Vec<crate::models::Device>>> {
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !authorized(&state.console, token) {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "Invalid console access token",
        ));
    }
    Ok(axum::Json(state.storage.list_devices().await?))
}

pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if state.console.token.len() < 32 {
        return Err(ApiError::bad_request(
            "CONSOLE_DISABLED",
            "Configure the remote console first",
        ));
    }
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !state.config.allowed_origins.iter().any(|v| v == origin) {
        return Err(ApiError::bad_request(
            "ORIGIN_DENIED",
            "Console origin is not allowed",
        ));
    }
    Ok(ws.max_message_size(4096).on_upgrade(move |mut socket| async move {
        // Token travels in the first WSS message, never in URLs or access logs.
        let Ok(Some(Ok(Message::Text(first)))) = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await else { return };
        let Ok(hello) = serde_json::from_str::<Hello>(&first) else { return };
        if !authorized(&state.console, &hello.token) { return; }
        if state.storage.get_device(&hello.device_id).await.is_err() { return; }
        if socket.send(Message::Text(r#"{"notice":"Authenticated; live stream ready"}"#.into())).await.is_err() { return; }
        // Snapshot and subscribe under the ingestion lock to avoid duplicate boundary events.
        let (mut events, history) = {
            let history = state.console.history.lock().await;
            (state.console.events.subscribe(), history.get(&hello.device_id).cloned().unwrap_or_default())
        };
        for entry in history {
            let Ok(json) = serde_json::to_string(&entry) else { continue };
            if socket.send(Message::Text(json.into())).await.is_err() { return; }
        }
        let mut ping = tokio::time::interval(Duration::from_secs(20));
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(entry) if entry.device_id == hello.device_id => {
                        let Ok(json) = serde_json::to_string(&entry) else { continue };
                        if socket.send(Message::Text(json.into())).await.is_err() { break; }
                    },
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let notice = serde_json::json!({"notice":format!("Slow viewer: skipped {n} events")});
                        if socket.send(Message::Text(notice.to_string().into())).await.is_err() { break; }
                    },
                    Err(_) => break,
                    _ => {}
                },
                incoming = socket.recv() => match incoming {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    _ => {}
                },
                _ = ping.tick() => {
                    if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tls_provider_is_available() {
        // Both SQL and MQTT use Rustls; feature unification must not make provider selection panic.
        let _ = rustls::ClientConfig::builder();
    }
    #[tokio::test]
    async fn rejects_spoofed_identity_and_bounds_history() {
        let console = Console::new();
        let payload = br#"{"device_id":"node","level":"INFO","message":"hello"}"#;
        console.accept("devices/other/logs", payload).await;
        assert!(console.history.lock().await.is_empty());
        for _ in 0..250 {
            console.accept("devices/node/logs", payload).await;
        }
        assert_eq!(console.history.lock().await["node"].len(), 200);
    }

    #[tokio::test]
    async fn broker_state_changes_notify_reporting_without_duplicate_events() {
        let temp = tempfile::tempdir().expect("test directory");
        let storage = crate::storage::Storage::connect(&format!(
            "sqlite:{}?mode=rwc",
            temp.path().join("test.db").display()
        ))
        .await
        .expect("database");
        let tinyml = crate::tinyml::TinyMl::new(
            &storage,
            crate::tinyml::Settings {
                enabled: true,
                prefix: "tinyml/v1".into(),
                max_bytes: 65536,
                timeout_seconds: 300,
            },
        );
        let console = Console::for_test("a".repeat(64));
        let mut events = tinyml.events.subscribe();
        assert!(!console.mqtt_connected());
        console.set_mqtt_connected(true, &tinyml);
        assert!(console.mqtt_connected());
        assert_eq!(events.try_recv().expect("connection event")["connected"], true);
        console.set_mqtt_connected(true, &tinyml);
        assert!(events.try_recv().is_err());
        console.set_mqtt_connected(false, &tinyml);
        assert_eq!(events.try_recv().expect("disconnection event")["connected"], false);
    }
}
