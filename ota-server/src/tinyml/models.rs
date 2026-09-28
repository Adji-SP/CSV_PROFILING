use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Run {
    pub id: String,
    pub device_id: String,
    pub run_id: String,
    pub run_name: Option<String>,
    pub mode: Option<String>,
    pub status: String,
    pub final_status: Option<String>,
    pub started_at_device: Option<String>,
    pub started_at_server: String,
    pub completed_at_device: Option<String>,
    pub completed_at_server: Option<String>,
    pub last_received_at: String,
    pub start_json: String,
    pub complete_json: String,
    pub other_json: String,
    pub samples: i64,
    pub labelled: i64,
    pub correct_count: i64,
    pub confidence_sum: f64,
    pub confidence_count: i64,
    pub latency_sum_us: f64,
    pub latency_count: i64,
}
impl Run {
    pub fn view(&self) -> Value {
        let start = json_value(&self.start_json);
        json!({"id":self.id,"device_id":self.device_id,"run_id":self.run_id,"run_name":self.run_name,"mode":self.mode,
            "status":self.status,"started_at_device":self.started_at_device,"started_at_server":self.started_at_server,
            "completed_at_device":self.completed_at_device,"completed_at_server":self.completed_at_server,"last_received_at":self.last_received_at,
            "samples":self.samples,"labelled_samples":self.labelled,"accuracy":ratio(self.correct_count as f64,self.labelled),
            "mean_confidence":ratio(self.confidence_sum,self.confidence_count),"mean_inference_ms":ratio(self.latency_sum_us / 1000.0,self.latency_count),
            "model":start.get("model"),"device":start.get("device"),"configuration":start.get("configuration"),"other":json_value(&self.other_json),
            "duration_seconds":duration(&self.started_at_server,self.completed_at_server.as_deref().unwrap_or(&self.last_received_at))})
    }
}
pub fn ratio(n: f64, d: i64) -> Option<f64> {
    (d > 0).then(|| n / d as f64)
}
pub fn duration(start: &str, end: &str) -> Option<f64> {
    let a = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let b = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    let value = (b - a).num_milliseconds() as f64 / 1000.0;
    (value >= 0.0).then_some(value)
}
pub fn json_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ResultRow {
    pub id: String,
    pub run_key: String,
    pub message_id: String,
    pub batch_index: i64,
    pub sequence: Option<i64>,
    pub sample_id: Option<String>,
    pub device_timestamp: Option<String>,
    pub server_timestamp: String,
    pub actual_class: Option<String>,
    pub predicted_class: Option<String>,
    pub confidence: Option<f64>,
    pub correct: Option<i64>,
    pub inference_us: Option<f64>,
    pub total_pipeline_us: Option<f64>,
    pub preprocessing_us: Option<f64>,
    pub postprocessing_us: Option<f64>,
    pub free_heap_bytes: Option<i64>,
    pub minimum_free_heap_bytes: Option<i64>,
    pub chip_temperature_c: Option<f64>,
    pub other_json: String,
    pub data_json: String,
}
impl ResultRow {
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.remove("data_json");
            o.remove("other_json");
            o.insert("data".into(), json_value(&self.data_json));
            o.insert("other".into(), json_value(&self.other_json));
            o.insert(
                "correct".into(),
                self.correct
                    .map(|n| Value::Bool(n == 1))
                    .unwrap_or(Value::Null),
            );
        }
        v
    }
}
#[derive(Default, Debug, Clone, Deserialize)]
pub struct Filters {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub actual_class: Option<String>,
    pub predicted_class: Option<String>,
    pub correct: Option<bool>,
    pub min_confidence: Option<f64>,
    pub max_confidence: Option<f64>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub device_id: Option<String>,
    pub status: Option<String>,
}
impl Filters {
    pub fn pagination(&self) -> crate::errors::ApiResult<(i64, i64)> {
        let page = self.page.unwrap_or(1);
        let size = self.page_size.unwrap_or(50);
        if !(1..=1_000_000).contains(&page)
            || !(1..=200).contains(&size)
            || self
                .min_confidence
                .is_some_and(|n| !(0.0..=1.0).contains(&n))
            || self
                .max_confidence
                .is_some_and(|n| !(0.0..=1.0).contains(&n))
        {
            return Err(super::protocol::invalid(
                "Invalid pagination or confidence range",
            ));
        }
        if self
            .min_confidence
            .zip(self.max_confidence)
            .is_some_and(|(a, b)| a > b)
        {
            return Err(super::protocol::invalid(
                "min_confidence exceeds max_confidence",
            ));
        }
        Ok((page, size))
    }
}
