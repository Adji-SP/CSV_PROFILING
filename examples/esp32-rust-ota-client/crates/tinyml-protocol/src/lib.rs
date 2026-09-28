//! Shared wire types. Experimental fields stay in `other`; units are explicit.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    RunStart,
    InferenceResult,
    InferenceBatch,
    MetricsSnapshot,
    Event,
    RunComplete,
    Status,
}
impl MessageType {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::RunStart => "start",
            Self::InferenceResult | Self::InferenceBatch => "result",
            Self::MetricsSnapshot => "metrics",
            Self::Event => "event",
            Self::RunComplete => "complete",
            Self::Status => "status",
        }
    }
    pub fn topic(self, prefix: &str, device: &str, run: Option<&str>) -> String {
        match self {
            Self::RunStart => format!("{prefix}/{device}/run/start"),
            Self::Status => format!("{prefix}/{device}/status"),
            _ => format!(
                "{prefix}/{device}/run/{}/{}",
                run.unwrap_or_default(),
                self.suffix()
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub schema_version: String,
    pub message_id: String,
    pub message_type: MessageType,
    pub device_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub sequence: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub other: Value,
}

// Firmware can explicitly report unknown measurements without inventing zeros.
pub fn optional<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let v = Value::deserialize(d)?;
    if v.is_null() || v.as_str() == Some("unknown") {
        return Ok(None);
    }
    serde_json::from_value(v)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Model {
    pub name: Option<String>,
    pub version: Option<String>,
    pub model_format: Option<String>,
    pub quantization: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    pub model_size_bytes: Option<u64>,
    pub input_shape: Option<Vec<u64>>,
    pub output_shape: Option<Vec<u64>>,
    pub class_names: Option<Vec<String>>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceMetadata {
    pub board: Option<String>,
    pub firmware_version: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    pub cpu_frequency_mhz: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub flash_total_bytes: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub ram_total_bytes: Option<u64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Configuration {
    #[serde(default, deserialize_with = "optional")]
    pub confidence_threshold: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub expected_samples: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub sampling_period_ms: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Start {
    pub run_name: Option<String>,
    pub mode: Option<String>,
    pub model: Option<Model>,
    pub device: Option<DeviceMetadata>,
    pub configuration: Option<Configuration>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timing {
    #[serde(default, deserialize_with = "optional")]
    pub preprocessing_us: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub inference_us: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub postprocessing_us: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub total_pipeline_us: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct System {
    #[serde(default, deserialize_with = "optional")]
    pub free_heap_bytes: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub minimum_free_heap_bytes: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub chip_temperature_c: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Inference {
    #[serde(default)]
    pub sample_id: Value,
    #[serde(default, deserialize_with = "optional")]
    pub actual_class: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    pub predicted_class: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    pub confidence: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub correct: Option<bool>,
    pub timing: Option<Timing>,
    pub system: Option<System>,
    // Also accept the compact batch shape, normalized by the backend.
    #[serde(default, deserialize_with = "optional")]
    pub inference_us: Option<f64>,
    #[serde(default)]
    pub other: Value,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    #[serde(default, deserialize_with = "optional")]
    pub free_heap_bytes: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub minimum_free_heap_bytes: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub tensor_arena_bytes: Option<u64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Thermal {
    #[serde(default, deserialize_with = "optional")]
    pub chip_temperature_c: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reliability {
    #[serde(default, deserialize_with = "optional")]
    pub dropped_samples: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub inference_errors: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub processing_errors: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub reset_count: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    pub telemetry_dropped: Option<u64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotTiming {
    #[serde(default, deserialize_with = "optional")]
    pub average_inference_ms: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub minimum_inference_ms: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub maximum_inference_ms: Option<f64>,
    #[serde(default, deserialize_with = "optional")]
    pub average_pipeline_ms: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Runtime {
    #[serde(default, deserialize_with = "optional")]
    pub uptime_ms: Option<u64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default, deserialize_with = "optional")]
    pub samples_processed: Option<u64>,
    pub timing: Option<SnapshotTiming>,
    pub memory: Option<Memory>,
    pub thermal: Option<Thermal>,
    pub reliability: Option<Reliability>,
    pub runtime: Option<Runtime>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Complete {
    pub status: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    pub samples_processed: Option<u64>,
    pub reliability: Option<Reliability>,
    pub final_system_state: Option<System>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Event {
    pub level: Option<String>,
    pub message: Option<String>,
    pub code: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub status: Option<String>,
    pub firmware_version: Option<String>,
    pub runtime: Option<Runtime>,
}
