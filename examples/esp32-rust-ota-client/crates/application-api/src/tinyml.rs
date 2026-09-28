//! Nonblocking telemetry API. MQTT transport belongs to the managed runtime.
use serde::Serialize;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc::{sync_channel, Receiver, SyncSender},
    OnceLock,
};
pub use tinyml_protocol::*;
static QUEUE: OnceLock<SyncSender<Packet>> = OnceLock::new();
static DROPPED: AtomicU32 = AtomicU32::new(0);
pub const MAX_PAYLOAD: usize = 4096;
pub struct Packet {
    pub topic: String,
    pub payload: Vec<u8>,
}
#[derive(Debug)]
pub enum PublishError {
    Disabled,
    QueueFull,
    InvalidId,
    TooLarge,
}
impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TinyML publish unavailable: {self:?}")
    }
}
impl std::error::Error for PublishError {}
#[doc(hidden)]
pub fn install() -> Option<Receiver<Packet>> {
    let (tx, rx) = sync_channel(8);
    QUEUE.set(tx).ok()?;
    Some(rx)
}
pub fn dropped_messages() -> u32 {
    DROPPED.load(Ordering::Relaxed)
}
/// Heap observations, not total SRAM capacity. Temperature is intentionally absent.
pub fn system_metrics() -> System {
    System {
        free_heap_bytes: Some(unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as u64),
        minimum_free_heap_bytes: Some(
            unsafe { esp_idf_svc::sys::esp_get_minimum_free_heap_size() } as u64,
        ),
        ..Default::default()
    }
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn random_id() -> String {
    // ESP-IDF hardware RNG; no wall clock required for uniqueness.
    unsafe {
        format!(
            "{:08x}{:08x}{:08x}{:08x}",
            esp_idf_svc::sys::esp_random(),
            esp_idf_svc::sys::esp_random(),
            esp_idf_svc::sys::esp_random(),
            esp_idf_svc::sys::esp_random()
        )
    }
}
pub struct Telemetry {
    device_id: String,
    run_id: String,
    prefix: String,
    sequence: i64,
}
impl Telemetry {
    pub fn new(device_id: &str, run_id: &str) -> Result<Self, PublishError> {
        if !valid_id(device_id) || !valid_id(run_id) {
            return Err(PublishError::InvalidId);
        }
        let prefix = option_env!("TINYML_MQTT_TOPIC_PREFIX").unwrap_or("tinyml/v1");
        if !prefix.split('/').all(valid_id) {
            return Err(PublishError::InvalidId);
        }
        Ok(Self {
            device_id: device_id.into(),
            run_id: run_id.into(),
            prefix: prefix.into(),
            sequence: 0,
        })
    }
    pub fn new_run_id() -> String {
        format!("run-{}", random_id())
    }
    fn publish<T: Serialize>(
        &mut self,
        kind: MessageType,
        data: T,
        other: Value,
    ) -> Result<(), PublishError> {
        let result = (|| {
            let queue = QUEUE.get().ok_or(PublishError::Disabled)?;
            let envelope = Envelope {
                schema_version: "1.0".into(),
                message_id: random_id(),
                message_type: kind,
                device_id: self.device_id.clone(),
                run_id: Some(self.run_id.clone()),
                sequence: Some(self.sequence),
                timestamp: None,
                data: serde_json::to_value(data).map_err(|_| PublishError::TooLarge)?,
                other,
            };
            self.sequence = self.sequence.saturating_add(1);
            struct Bounded(Vec<u8>);
            impl std::io::Write for Bounded {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    if self.0.len() + bytes.len() > MAX_PAYLOAD {
                        return Err(std::io::Error::other("Telemetry payload limit"));
                    }
                    self.0.extend_from_slice(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut payload = Bounded(Vec::with_capacity(512));
            serde_json::to_writer(&mut payload, &envelope).map_err(|_| PublishError::TooLarge)?;
            queue
                .try_send(Packet {
                    topic: kind.topic(&self.prefix, &self.device_id, Some(&self.run_id)),
                    payload: payload.0,
                })
                .map_err(|_| PublishError::QueueFull)
        })();
        if result.is_err() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
    pub fn start_run(&mut self, data: Start, other: Value) -> Result<(), PublishError> {
        self.publish(MessageType::RunStart, data, other)
    }
    pub fn publish_result(&mut self, data: Inference, other: Value) -> Result<(), PublishError> {
        self.publish(MessageType::InferenceResult, data, other)
    }
    pub fn publish_metrics(
        &mut self,
        mut data: Snapshot,
        other: Value,
    ) -> Result<(), PublishError> {
        data.reliability
            .get_or_insert_with(Reliability::default)
            .telemetry_dropped = Some(dropped_messages().into());
        self.publish(MessageType::MetricsSnapshot, data, other)
    }
    pub fn publish_event(&mut self, data: Event, other: Value) -> Result<(), PublishError> {
        self.publish(MessageType::Event, data, other)
    }
    pub fn complete_run(&mut self, mut data: Complete, other: Value) -> Result<(), PublishError> {
        data.reliability
            .get_or_insert_with(Reliability::default)
            .telemetry_dropped = Some(dropped_messages().into());
        self.publish(MessageType::RunComplete, data, other)
    }
}
