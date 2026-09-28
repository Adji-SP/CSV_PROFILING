use super::models::{ResultRow, Run, json_value, ratio};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Debug, Clone, Serialize)]
pub struct Statistics {
    pub count: usize,
    pub mean: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub median: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}
pub fn statistics(mut values: Vec<f64>) -> Statistics {
    values.retain(|v| v.is_finite());
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return Statistics::default();
    }
    let percentile = |p: f64| {
        let i = (values.len() - 1) as f64 * p;
        let low = i.floor() as usize;
        let high = i.ceil() as usize;
        values[low] + (values[high] - values[low]) * (i - low as f64)
    };
    Statistics {
        count: values.len(),
        mean: Some(values.iter().sum::<f64>() / values.len() as f64),
        min: values.first().copied(),
        max: values.last().copied(),
        median: Some(percentile(0.5)),
        p95: Some(percentile(0.95)),
        p99: Some(percentile(0.99)),
    }
}
#[derive(Debug, Serialize)]
pub struct ClassMetrics {
    pub class: String,
    pub support: i64,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}
#[derive(Debug, Serialize)]
pub struct Classification {
    pub samples: i64,
    pub labelled_samples: i64,
    pub correct: Option<i64>,
    pub incorrect: Option<i64>,
    pub accuracy: Option<f64>,
    pub precision_macro: Option<f64>,
    pub recall_macro: Option<f64>,
    pub f1_macro: Option<f64>,
    pub precision_weighted: Option<f64>,
    pub recall_weighted: Option<f64>,
    pub f1_weighted: Option<f64>,
    pub mean_confidence: Option<f64>,
    pub confidence_correct: Option<f64>,
    pub confidence_incorrect: Option<f64>,
}
#[derive(Debug, Serialize)]
pub struct TinyMlReport {
    pub report_version: &'static str,
    pub generated_at: String,
    pub run: Value,
    pub model: Value,
    pub device: Value,
    pub classification: Classification,
    pub class_metrics: Vec<ClassMetrics>,
    pub confusion_matrix: Value,
    pub embedded: Value,
    pub analysis: Vec<String>,
    pub other: Value,
}
#[derive(Default)]
pub struct Accumulator {
    samples: i64,
    labelled: i64,
    correct: i64,
    labels: BTreeSet<String>,
    matrix: BTreeMap<(String, String), i64>,
    inference: Vec<f64>,
    pipeline: Vec<f64>,
    pre: Vec<f64>,
    post: Vec<f64>,
    confidence: Vec<f64>,
    confidence_correct: Vec<f64>,
    confidence_incorrect: Vec<f64>,
    heap: Vec<f64>,
    heap_min: Vec<f64>,
    temp: Vec<f64>,
    tensor: Option<u64>,
    reliability: BTreeMap<String, u64>,
    other: BTreeMap<String, Value>,
    other_truncated: bool,
    deadline: Option<f64>,
    deadline_count: i64,
    deadline_misses: i64,
    latest_snapshot: Value,
}
impl Accumulator {
    pub fn new(run: &Run) -> Self {
        let start = json_value(&run.start_json);
        let other = json_value(&run.other_json);
        let labels = start
            .pointer("/model/class_names")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let deadline = start
            .pointer("/configuration/sampling_period_ms")
            .and_then(Value::as_f64)
            .or_else(|| other.get("sampling_period_ms").and_then(Value::as_f64))
            .filter(|v| *v > 0.0);
        Self {
            labels,
            deadline,
            ..Self::default()
        }
    }
    fn label(&mut self, s: &str) {
        self.labels.insert(s.to_owned());
    }
    pub fn add(&mut self, row: &ResultRow) {
        self.samples += 1;
        if let Some(p) = &row.predicted_class {
            self.label(p);
        }
        if let Some(a) = &row.actual_class {
            self.label(a);
        }
        if let Some((a, p)) = row.actual_class.as_ref().zip(row.predicted_class.as_ref()) {
            self.labelled += 1;
            let correct = a == p;
            self.correct += i64::from(correct);
            *self.matrix.entry((a.clone(), p.clone())).or_default() += 1;
            if let Some(c) = row.confidence {
                if correct {
                    self.confidence_correct.push(c)
                } else {
                    self.confidence_incorrect.push(c)
                }
            }
        }
        if let Some(c) = row.confidence {
            self.confidence.push(c)
        }
        for (value, list) in [
            (row.inference_us, &mut self.inference),
            (row.total_pipeline_us, &mut self.pipeline),
            (row.preprocessing_us, &mut self.pre),
            (row.postprocessing_us, &mut self.post),
        ] {
            if let Some(v) = value {
                list.push(v / 1000.0);
            }
        }
        if let Some(heap) = row.free_heap_bytes {
            self.heap.push(heap as f64);
            self.heap_min.push(heap as f64);
        }
        if let Some(heap) = row.minimum_free_heap_bytes {
            self.heap_min.push(heap as f64);
        }
        if let Some(t) = row.chip_temperature_c {
            self.temp.push(t);
        }
        if let Some((deadline, time)) = self.deadline.zip(row.total_pipeline_us) {
            self.deadline_count += 1;
            self.deadline_misses += i64::from(time / 1000.0 > deadline);
        }
        self.custom(&json_value(&row.other_json));
    }
    pub fn custom(&mut self, value: &Value) {
        if value.is_null() {
            return;
        }
        // Compact report examples only. Full structures remain in persisted messages/results.
        let mut entries = Vec::new();
        fn walk(value: &Value, path: String, out: &mut Vec<(String, Value)>, depth: usize) {
            if depth < 6 {
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        walk(
                            v,
                            format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                            out,
                            depth + 1,
                        );
                    }
                    return;
                }
            }
            out.push((path, value.clone()));
        }
        walk(value, String::new(), &mut entries, 0);
        for (key, v) in entries {
            if self.other.len() >= 256 && !self.other.contains_key(&key) {
                self.other_truncated = true;
                continue;
            }
            if v.to_string().len() > 2048 {
                self.other_truncated = true;
                continue;
            }
            self.other.insert(key, v);
        }
    }
    pub fn snapshot(&mut self, data: &Value, other: &Value) {
        self.latest_snapshot = data.clone();
        if let Some(v) = data
            .pointer("/memory/free_heap_bytes")
            .and_then(Value::as_f64)
        {
            self.heap.push(v);
            self.heap_min.push(v);
        }
        if let Some(v) = data
            .pointer("/memory/minimum_free_heap_bytes")
            .and_then(Value::as_f64)
        {
            self.heap_min.push(v);
        }
        if let Some(v) = data
            .pointer("/thermal/chip_temperature_c")
            .and_then(Value::as_f64)
        {
            self.temp.push(v);
        }
        if let Some(v) = data
            .pointer("/memory/tensor_arena_bytes")
            .and_then(Value::as_u64)
        {
            self.tensor = Some(v);
        }
        self.counters(data.get("reliability"));
        self.custom(other);
    }
    fn counters(&mut self, values: Option<&Value>) {
        if let Some(r) = values {
            for k in [
                "dropped_samples",
                "inference_errors",
                "processing_errors",
                "reset_count",
                "telemetry_dropped",
            ] {
                if let Some(v) = r.get(k).and_then(Value::as_u64) {
                    self.reliability
                        .entry(k.into())
                        .and_modify(|n| *n = (*n).max(v))
                        .or_insert(v);
                }
            }
        }
    }
    pub fn finish(mut self, run: &Run) -> TinyMlReport {
        let start = json_value(&run.start_json);
        let complete = json_value(&run.complete_json);
        self.counters(complete.pointer("/data/reliability"));
        if let Some(s) = complete.pointer("/data/final_system_state") {
            if let Some(v) = s.get("free_heap_bytes").and_then(Value::as_f64) {
                self.heap.push(v);
                self.heap_min.push(v);
            }
            if let Some(v) = s.get("minimum_free_heap_bytes").and_then(Value::as_f64) {
                self.heap_min.push(v);
            }
            if let Some(v) = s.get("chip_temperature_c").and_then(Value::as_f64) {
                self.temp.push(v);
            }
        }
        let mut supports: BTreeMap<&str, i64> = BTreeMap::new();
        let mut predictions: BTreeMap<&str, i64> = BTreeMap::new();
        for ((a, p), n) in &self.matrix {
            *supports.entry(a).or_default() += n;
            *predictions.entry(p).or_default() += n;
        }
        let classes: Vec<ClassMetrics> = self
            .labels
            .iter()
            .map(|label| {
                let tp = *self
                    .matrix
                    .get(&(label.clone(), label.clone()))
                    .unwrap_or(&0);
                let support = *supports.get(label.as_str()).unwrap_or(&0);
                let predicted = *predictions.get(label.as_str()).unwrap_or(&0);
                ClassMetrics {
                    class: label.clone(),
                    support,
                    precision: ratio(tp as f64, predicted),
                    recall: ratio(tp as f64, support),
                    f1: ratio(2.0 * tp as f64, support + predicted),
                }
            })
            .collect();
        let macro_avg = |select: fn(&ClassMetrics) -> Option<f64>| {
            let values: Vec<f64> = classes.iter().filter_map(select).collect();
            ratio(values.iter().sum(), values.len() as i64)
        };
        // Undefined metrics for a supported class make that weighted metric unavailable.
        let weighted = |select: fn(&ClassMetrics) -> Option<f64>| {
            let mut sum = 0.0;
            for c in &classes {
                if c.support > 0 {
                    sum += select(c)? * c.support as f64;
                }
            }
            ratio(sum, self.labelled)
        };
        let classification = Classification {
            samples: self.samples,
            labelled_samples: self.labelled,
            correct: (self.labelled > 0).then_some(self.correct),
            incorrect: (self.labelled > 0).then_some(self.labelled - self.correct),
            accuracy: ratio(self.correct as f64, self.labelled),
            precision_macro: macro_avg(|c| c.precision),
            recall_macro: macro_avg(|c| c.recall),
            f1_macro: macro_avg(|c| c.f1),
            precision_weighted: weighted(|c| c.precision),
            recall_weighted: weighted(|c| c.recall),
            f1_weighted: weighted(|c| c.f1),
            mean_confidence: statistics(self.confidence).mean,
            confidence_correct: statistics(self.confidence_correct).mean,
            confidence_incorrect: statistics(self.confidence_incorrect).mean,
        };
        let confusion = if self.labelled == 0 {
            Value::Null
        } else if self.labels.len() <= 128 {
            let matrix: Vec<Vec<i64>> = self
                .labels
                .iter()
                .map(|a| {
                    self.labels
                        .iter()
                        .map(|p| *self.matrix.get(&(a.clone(), p.clone())).unwrap_or(&0))
                        .collect()
                })
                .collect();
            json!({"labels":self.labels,"matrix":matrix})
        } else {
            json!({"labels":self.labels,"sparse":self.matrix.iter().map(|((a,p),n)|json!({"actual":a,"predicted":p,"count":n})).collect::<Vec<_>>()})
        };
        let inf = statistics(self.inference);
        let pipe = statistics(self.pipeline);
        let heap = statistics(self.heap);
        let min_heap = statistics(self.heap_min).min;
        let temp = statistics(self.temp);
        let metadata = |p: &str| start.pointer(p).and_then(Value::as_f64);
        let percent = |a: Option<f64>, b: Option<f64>| {
            a.zip(b)
                .filter(|(_, d)| *d > 0.0)
                .map(|(n, d)| 100.0 * n / d)
        };
        let seconds = super::models::duration(
            &run.started_at_server,
            run.completed_at_server
                .as_deref()
                .unwrap_or(&run.last_received_at),
        );
        let throughput = seconds
            .filter(|s| *s > 0.0)
            .map(|s| self.samples as f64 / s);
        let mut findings = Vec::new();
        if self.labelled > 0 {
            findings.push(format!(
                "{} misclassifications from {} labelled samples.",
                self.labelled - self.correct,
                self.labelled
            ));
        }
        if let Some(p95) = inf.p95 {
            findings.push(format!("P95 inference latency was {p95:.3} ms."));
        }
        if let Some((max, median)) = inf
            .max
            .zip(inf.median)
            .filter(|(m, d)| *d > 0.0 && *m > 3.0 * *d)
        {
            findings.push(format!("Maximum inference latency ({max:.3} ms) exceeded three times the median ({median:.3} ms)."));
        }
        if let Some(v) = min_heap {
            findings.push(format!("Minimum observed free heap was {v:.0} bytes."));
        }
        if let Some(v) = temp.max {
            findings.push(format!("Maximum recorded chip temperature was {v:.2} °C."));
        }
        let mut reliability = json!({"dropped_samples":null,"inference_errors":null,"processing_errors":null,"reset_count":null,"telemetry_dropped":null});
        for (key, v) in self.reliability {
            reliability[&key] = json!(v);
        }
        let mut run_view = run.view();
        if run.status == "completing" {
            run_view["status"] = json!(run.final_status.as_deref().unwrap_or("completed"));
        }
        TinyMlReport {
            report_version: "1.0",
            generated_at: crate::storage::now(),
            run: run_view,
            model: start.get("model").cloned().unwrap_or(Value::Null),
            device: start.get("device").cloned().unwrap_or(Value::Null),
            classification,
            class_metrics: classes,
            confusion_matrix: confusion,
            embedded: json!({"inference_ms":inf,"pipeline_ms":pipe,"preprocessing_ms":statistics(self.pre),"postprocessing_ms":statistics(self.post),
                "latest_device_snapshot":self.latest_snapshot,"measurement_basis":"Latency and classification use received inference rows; memory and thermal include rows, snapshots and final state.",
                "throughput_inferences_per_second":throughput,"throughput_basis":"received samples / server-observed duration",
                "memory":{"average_free_heap_bytes":heap.mean,"minimum_free_heap_bytes":min_heap,"ram_utilization_percent":percent(metadata("/device/ram_total_bytes").zip(min_heap).filter(|(total,free)|total>=free).map(|(a,b)|a-b),metadata("/device/ram_total_bytes")),"tensor_arena_bytes":self.tensor,"tensor_arena_percent":percent(self.tensor.map(|n|n as f64),metadata("/device/ram_total_bytes"))},
                "model_size_bytes":metadata("/model/model_size_bytes"),"model_flash_percent":percent(metadata("/model/model_size_bytes"),metadata("/device/flash_total_bytes")),
                "thermal":{"average_chip_temperature_c":temp.mean,"minimum_chip_temperature_c":temp.min,"maximum_chip_temperature_c":temp.max},"reliability":reliability,
                "deadlines":{"sampling_period_ms":self.deadline,"observed_pipelines":self.deadline_count,"misses":(self.deadline_count>0).then_some(self.deadline_misses),"miss_percent":ratio(100.0*self.deadline_misses as f64,self.deadline_count),"real_time_capable":(self.deadline_count>0).then_some(self.deadline_misses==0)}}),
            analysis: findings,
            other: json!({"run":json_value(&run.other_json),"complete":complete.get("other"),"observed_values_by_json_pointer":self.other,"examples_truncated":self.other_truncated}),
        }
    }
}
