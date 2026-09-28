use crate::errors::{ApiError, ApiResult};
use serde::de::DeserializeOwned;
use serde_json::Value;
pub use tinyml_protocol::*;

#[derive(Debug)]
pub enum Data {
    Start(Start),
    Results(Vec<Inference>),
    Snapshot,
    Complete(Complete),
    Event,
    Status,
}
pub struct Parsed {
    pub envelope: Envelope,
    pub data: Data,
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
pub fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("TINYML_PROTOCOL", message)
}
fn decode<T: DeserializeOwned>(v: &Value) -> ApiResult<T> {
    serde_json::from_value(if v.is_null() {
        serde_json::json!({})
    } else {
        v.clone()
    })
    .map_err(|e| invalid(format!("Invalid message data: {e}")))
}
fn result(v: &Value) -> ApiResult<Inference> {
    let mut r: Inference = decode(v)?;
    if !r.sample_id.is_null() && !r.sample_id.is_string() && !r.sample_id.is_number() {
        return Err(invalid("sample_id must be a string or number"));
    }
    for s in [&r.actual_class, &r.predicted_class].into_iter().flatten() {
        if s.is_empty() || s.len() > 256 {
            return Err(invalid("Class labels must contain 1–256 bytes"));
        }
    }
    if r.confidence.is_some_and(|c| !(0.0..=1.0).contains(&c)) {
        return Err(invalid("confidence must be between 0 and 1"));
    }
    let timing = r.timing.get_or_insert_with(Timing::default);
    if timing.inference_us.is_none() {
        timing.inference_us = r.inference_us;
    }
    for v in [
        timing.inference_us,
        timing.preprocessing_us,
        timing.postprocessing_us,
        timing.total_pipeline_us,
    ]
    .into_iter()
    .flatten()
    {
        if !v.is_finite() || v < 0.0 {
            return Err(invalid("Timing values must be finite and nonnegative"));
        }
    }
    // Ground truth, when available, is authoritative. Device `correct` is advisory only.
    r.correct = r
        .actual_class
        .as_ref()
        .zip(r.predicted_class.as_ref())
        .map(|(a, p)| a == p);
    Ok(r)
}
pub fn parse(topic: &str, bytes: &[u8], prefix: &str, max_bytes: usize) -> ApiResult<Parsed> {
    if bytes.len() > max_bytes {
        return Err(ApiError::payload_too_large(max_bytes));
    }
    let raw: Value = serde_json::from_slice(bytes).map_err(|_| invalid("Invalid UTF-8 or JSON"))?;
    if raw.get("timestamp").is_none() {
        return Err(invalid(
            "timestamp is required; use null if the clock is unknown",
        ));
    }
    let e: Envelope =
        serde_json::from_value(raw).map_err(|e| invalid(format!("Invalid envelope: {e}")))?;
    if e.schema_version != "1.0" {
        return Err(invalid("Unsupported schema_version; supported: 1.0"));
    }
    if !valid_id(&e.device_id) || !valid_id(&e.message_id) {
        return Err(invalid("Invalid device_id or message_id"));
    }
    if e.sequence.is_some_and(|n| n < 0) {
        return Err(invalid("sequence cannot be negative"));
    }
    if let Some(t) = &e.timestamp {
        chrono::DateTime::parse_from_rfc3339(t)
            .map_err(|_| invalid("timestamp must be RFC3339 or null"))?;
    }
    if e.message_type != MessageType::Status && !e.run_id.as_deref().is_some_and(valid_id) {
        return Err(invalid("run_id is required"));
    }
    if topic
        != e.message_type
            .topic(prefix, &e.device_id, e.run_id.as_deref())
    {
        return Err(invalid("MQTT topic and envelope identity/type disagree"));
    }
    let data = match e.message_type {
        MessageType::RunStart => {
            let start: Start = decode(&e.data)?;
            if start
                .mode
                .as_deref()
                .is_some_and(|m| !["evaluation", "deployment", "unknown"].contains(&m))
            {
                return Err(invalid("Invalid mode"));
            }
            Data::Start(start)
        }
        MessageType::InferenceResult => Data::Results(vec![result(&e.data)?]),
        MessageType::InferenceBatch => {
            let rows = e
                .data
                .get("results")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("results must be an array"))?;
            if rows.is_empty() || rows.len() > 256 {
                return Err(invalid("Batch requires 1–256 results"));
            }
            Data::Results(rows.iter().map(result).collect::<ApiResult<_>>()?)
        }
        MessageType::MetricsSnapshot => {
            let _: Snapshot = decode(&e.data)?;
            Data::Snapshot
        }
        MessageType::RunComplete => {
            let mut c: Complete = decode(&e.data)?;
            let state = match c.status.as_deref().unwrap_or("completed") {
                "completed" => "completed",
                "failed" => "failed",
                "cancelled" | "aborted" => "cancelled",
                "timeout" | "timed_out" => "timed_out",
                _ => return Err(invalid("Invalid completion status")),
            };
            c.status = Some(state.to_owned());
            Data::Complete(c)
        }
        MessageType::Event => {
            let _: Event = decode(&e.data)?;
            Data::Event
        }
        MessageType::Status => {
            let _: Status = decode(&e.data)?;
            Data::Status
        }
    };
    Ok(Parsed { envelope: e, data })
}
