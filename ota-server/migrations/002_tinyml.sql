CREATE TABLE IF NOT EXISTS tinyml_runs (
 id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(device_id), run_id TEXT NOT NULL,
 run_name TEXT, mode TEXT, status TEXT NOT NULL, final_status TEXT,
 started_at_device TEXT, started_at_server TEXT NOT NULL, completed_at_device TEXT, completed_at_server TEXT,
 last_received_at TEXT NOT NULL, start_json TEXT NOT NULL DEFAULT '{}', complete_json TEXT NOT NULL DEFAULT '{}',
 other_json TEXT NOT NULL DEFAULT 'null',
 samples BIGINT NOT NULL DEFAULT 0, labelled BIGINT NOT NULL DEFAULT 0, correct_count BIGINT NOT NULL DEFAULT 0,
 confidence_sum DOUBLE PRECISION NOT NULL DEFAULT 0, confidence_count BIGINT NOT NULL DEFAULT 0,
 latency_sum_us DOUBLE PRECISION NOT NULL DEFAULT 0, latency_count BIGINT NOT NULL DEFAULT 0,
 UNIQUE(device_id,run_id)
);
CREATE TABLE IF NOT EXISTS tinyml_messages (
 message_id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(device_id), run_key TEXT REFERENCES tinyml_runs(id),
 message_type TEXT NOT NULL, topic TEXT NOT NULL, received_at TEXT NOT NULL, payload_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tinyml_results (
 id TEXT PRIMARY KEY, run_key TEXT NOT NULL REFERENCES tinyml_runs(id), message_id TEXT NOT NULL REFERENCES tinyml_messages(message_id),
 batch_index BIGINT NOT NULL, sequence BIGINT, sample_id TEXT,
 device_timestamp TEXT, server_timestamp TEXT NOT NULL,
 actual_class TEXT, predicted_class TEXT, confidence DOUBLE PRECISION, correct BIGINT,
 inference_us DOUBLE PRECISION, total_pipeline_us DOUBLE PRECISION,
 preprocessing_us DOUBLE PRECISION, postprocessing_us DOUBLE PRECISION,
 free_heap_bytes BIGINT, minimum_free_heap_bytes BIGINT, chip_temperature_c DOUBLE PRECISION,
 other_json TEXT NOT NULL, data_json TEXT NOT NULL,
 UNIQUE(message_id,batch_index)
);
CREATE TABLE IF NOT EXISTS tinyml_metric_snapshots (
 id TEXT PRIMARY KEY, run_key TEXT NOT NULL REFERENCES tinyml_runs(id), message_id TEXT NOT NULL UNIQUE REFERENCES tinyml_messages(message_id),
 sequence BIGINT, device_timestamp TEXT, server_timestamp TEXT NOT NULL, data_json TEXT NOT NULL, other_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tinyml_events (
 id TEXT PRIMARY KEY, run_key TEXT REFERENCES tinyml_runs(id), device_id TEXT NOT NULL REFERENCES devices(device_id),
 message_id TEXT NOT NULL UNIQUE REFERENCES tinyml_messages(message_id), message_type TEXT NOT NULL,
 device_timestamp TEXT, server_timestamp TEXT NOT NULL, data_json TEXT NOT NULL, other_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tinyml_reports (
 run_key TEXT PRIMARY KEY REFERENCES tinyml_runs(id), generated_at TEXT NOT NULL, report_version TEXT NOT NULL, report_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tinyml_runs_activity ON tinyml_runs(status,last_received_at);
CREATE INDEX IF NOT EXISTS tinyml_results_run_time ON tinyml_results(run_key,server_timestamp,id);
CREATE INDEX IF NOT EXISTS tinyml_results_sample ON tinyml_results(run_key,sample_id);
CREATE INDEX IF NOT EXISTS tinyml_results_classes ON tinyml_results(run_key,actual_class,predicted_class);
CREATE INDEX IF NOT EXISTS tinyml_snapshots_run ON tinyml_metric_snapshots(run_key,server_timestamp);
CREATE INDEX IF NOT EXISTS tinyml_events_run ON tinyml_events(run_key,server_timestamp);
