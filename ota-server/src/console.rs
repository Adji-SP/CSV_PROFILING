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
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS, Transport};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
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
    history: Mutex<HashMap<String, VecDeque<LogEntry>>>,
    events: broadcast::Sender<LogEntry>,
}

impl Console {
    #[cfg(test)]
    pub(crate) fn for_test(token: String) -> Arc<Self> {
        let (events, _) = broadcast::channel(512);
        Arc::new(Self {
            token,
            history: Mutex::new(HashMap::new()),
            events,
        })
    }
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(512);
        Arc::new(Self {
            token: std::env::var("CONSOLE_VIEWER_TOKEN").unwrap_or_default(),
            history: Mutex::new(HashMap::new()),
            events,
        })
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

    pub fn start_mqtt(self: &Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        let Ok(host) = std::env::var("MQTT_HOST") else {
            return Ok(());
        };
        if self.token.len() < 32 {
            return Err("CONSOLE_VIEWER_TOKEN must contain at least 32 characters".into());
        }
        let port = std::env::var("MQTT_PORT")
            .unwrap_or("8883".into())
            .parse()?;
        let mut options = MqttOptions::new(
            std::env::var("MQTT_CLIENT_ID").unwrap_or("ota-console-backend".into()),
            host,
            port,
        );
        options.set_keep_alive(Duration::from_secs(30));
        options.set_max_packet_size(8192, 8192);
        options.set_credentials(
            std::env::var("MQTT_USERNAME")?,
            std::env::var("MQTT_PASSWORD").unwrap_or_default(),
        );
        let ca = std::env::var("MQTT_CA_FILE").ok();
        let cert = std::env::var("MQTT_CERT_FILE").ok();
        let key = std::env::var("MQTT_KEY_FILE").ok();
        match (ca, cert, key) {
            (Some(ca), Some(cert), Some(key)) => {
                options.set_transport(Transport::tls(
                    std::fs::read(ca)?,
                    Some((std::fs::read(cert)?, std::fs::read(key)?)),
                    None,
                ));
            }
            (Some(ca), None, None) => {
                options.set_transport(Transport::tls(std::fs::read(ca)?, None, None));
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
        let console = self.clone();
        tokio::spawn(async move {
            loop {
                match events.poll().await {
                    Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                        if client
                            .subscribe("devices/+/logs", QoS::AtMostOnce)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Event::Incoming(Incoming::Publish(p))) if !p.retain => {
                        console.accept(&p.topic, &p.payload).await
                    }
                    Ok(_) => {}
                    Err(_) => {
                        tracing::warn!("Console MQTT connection lost; retrying");
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

fn authorized(console: &Console, token: &str) -> bool {
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
}
