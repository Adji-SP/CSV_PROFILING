use super::*;
use protocol::MessageType;
use serde_json::{Value, json};

fn payload(kind: MessageType, id: &str, data: Value) -> (String, Vec<u8>) {
    let e = protocol::Envelope {
        schema_version: "1.0".into(),
        message_id: id.into(),
        message_type: kind,
        device_id: "node".into(),
        run_id: Some("run-1".into()),
        sequence: None,
        timestamp: None,
        data,
        other: json!({"nested":{"flag":true,"vector":[1,"x",null]},"future":2.3}),
    };
    (
        kind.topic("tinyml/v1", "node", Some("run-1")),
        serde_json::to_vec(&e).expect("json"),
    )
}
#[test]
fn protocol_optional_fields_extensions_and_validation() {
    for kind in [
        MessageType::RunStart,
        MessageType::InferenceResult,
        MessageType::MetricsSnapshot,
        MessageType::RunComplete,
        MessageType::Event,
        MessageType::Status,
    ] {
        let (t, p) = payload(kind, "msg", json!({}));
        let parsed = protocol::parse(&t, &p, "tinyml/v1", 65536).expect("minimal payload");
        assert_eq!(
            parsed.envelope.other["nested"]["vector"],
            json!([1, "x", null])
        );
    }
    let (t, p) = payload(
        MessageType::InferenceResult,
        "m",
        json!({"predicted_class":"A","confidence":0.5,"actual_class":null,"correct":true,"timing":{"inference_us":"unknown"}}),
    );
    let parsed = protocol::parse(&t, &p, "tinyml/v1", 65536).expect("deployment");
    if let protocol::Data::Results(rows) = parsed.data {
        assert_eq!(rows[0].correct, None);
    } else {
        panic!("result");
    }
    assert!(protocol::parse(&t, b"\xff", "tinyml/v1", 65536).is_err());
    assert!(protocol::parse(&t, b"{", "tinyml/v1", 65536).is_err());
    assert!(protocol::parse(&t, &p, "tinyml/v1", 2).is_err());
    let mut v: Value = serde_json::from_slice(&p).expect("json");
    v["schema_version"] = json!("2.0");
    assert!(protocol::parse(&t, v.to_string().as_bytes(), "tinyml/v1", 65536).is_err());
    v["schema_version"] = json!("1.0");
    v["device_id"] = json!("spoofed");
    assert!(protocol::parse(&t, v.to_string().as_bytes(), "tinyml/v1", 65536).is_err());
    let (t, p) = payload(MessageType::InferenceResult, "m", json!({"confidence":1.1}));
    assert!(protocol::parse(&t, &p, "tinyml/v1", 65536).is_err());
    let (t, p) = payload(
        MessageType::InferenceResult,
        "m",
        json!({"system":{"free_heap_bytes":"wrong"}}),
    );
    assert!(protocol::parse(&t, &p, "tinyml/v1", 65536).is_err());
}
#[test]
fn deterministic_percentiles_and_empty_single_sample() {
    let s = metrics::statistics((1..=100).map(f64::from).collect());
    assert_eq!(s.median, Some(50.5));
    assert_eq!(s.p95, Some(95.05));
    assert_eq!(s.p99, Some(99.01));
    assert_eq!(s.min, Some(1.0));
    assert_eq!(s.max, Some(100.0));
    assert_eq!(metrics::statistics(vec![]).mean, None);
    assert_eq!(metrics::statistics(vec![4.0]).p99, Some(4.0));
}
async fn fixture() -> (tempfile::TempDir, std::sync::Arc<TinyMl>) {
    let dir = tempfile::tempdir().expect("temp");
    let storage = crate::storage::Storage::connect(&format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("test.db").display()
    ))
    .await
    .expect("storage");
    storage
        .upsert_device("node", "Node", "esp32s3", "1", "127.0.0.1")
        .await
        .expect("device");
    let s = TinyMl::new(
        &storage,
        Settings {
            enabled: true,
            prefix: "tinyml/v1".into(),
            max_bytes: 65536,
            timeout_seconds: 300,
        },
    );
    (dir, s)
}
async fn send(s: &TinyMl, kind: MessageType, id: &str, data: Value) -> bool {
    let (t, p) = payload(kind, id, data);
    s.ingest(&t, &p).await.expect("ingest")
}
async fn run_id(s: &TinyMl) -> String {
    s.repo
        .runs(&models::Filters::default())
        .await
        .expect("runs")["items"][0]["id"]
        .as_str()
        .expect("id")
        .into()
}
#[tokio::test]
async fn lifecycle_dedup_filters_report_and_custom_json() {
    let (_dir, s) = fixture().await;
    assert!(
        send(
            &s,
            MessageType::RunStart,
            "start",
            json!({"mode":"evaluation","model":{"class_names":["A","B"]}})
        )
        .await
    );
    for (i, (a, p)) in [("A", "A"), ("A", "B"), ("B", "B"), ("B", "B")]
        .iter()
        .enumerate()
    {
        let data = json!({"sample_id":i,"actual_class":a,"predicted_class":p,"confidence":0.9,"timing":{"inference_us":(i+1)*1000}});
        assert!(
            send(
                &s,
                MessageType::InferenceResult,
                &format!("r{i}"),
                data.clone()
            )
            .await
        );
        assert!(!send(&s, MessageType::InferenceResult, &format!("r{i}"), data).await);
    }
    let id = run_id(&s).await;
    let filtered = s
        .repo
        .results(
            &id,
            &models::Filters {
                correct: Some(false),
                page_size: Some(1),
                ..Default::default()
            },
        )
        .await
        .expect("filter");
    assert_eq!(filtered["total"], 1);
    assert_eq!(
        filtered["items"][0]["other"]["nested"]["vector"],
        json!([1, "x", null])
    );
    assert!(send(&s, MessageType::RunComplete, "end", json!({})).await);
    s.maintain().await.expect("report");
    let report = s.repo.report(&id).await.expect("persisted report");
    assert_eq!(report["classification"]["accuracy"], 0.75);
    assert_eq!(report["classification"]["correct"], 3);
    assert_eq!(
        report["confusion_matrix"]["matrix"],
        json!([[1, 1], [0, 2]])
    );
    assert_eq!(report["class_metrics"][0]["precision"], 1.0);
    assert_eq!(report["class_metrics"][0]["recall"], 0.5);
    assert!((report["class_metrics"][0]["f1"].as_f64().expect("f1") - 2.0 / 3.0).abs() < 1e-8);
    assert_eq!(report["class_metrics"][1]["support"], 2);
    assert!(!send(&s, MessageType::RunComplete, "end", json!({})).await);
    assert!(!send(&s, MessageType::RunComplete, "end-again", json!({})).await);
    let (t, p) = payload(MessageType::RunStart, "restart", json!({}));
    assert!(s.ingest(&t, &p).await.is_err());
    assert_eq!(s.repo.run(&id).await.expect("run").status, "completed");
    s.maintain().await.expect("idempotent maintenance");
    assert_eq!(
        s.repo.report(&id).await.expect("report")["generated_at"],
        report["generated_at"]
    );
}
#[tokio::test]
async fn deployment_batch_timeout_and_empty_report() {
    let (_dir, s) = fixture().await;
    send(
        &s,
        MessageType::RunStart,
        "start",
        json!({"mode":"deployment"}),
    )
    .await;
    send(&s,MessageType::InferenceBatch,"batch",json!({"results":[{"sample_id":1,"predicted_class":"X","inference_us":1200},{"sample_id":2,"predicted_class":"Y"}]})).await;
    let id = run_id(&s).await;
    sqlx::query("UPDATE tinyml_runs SET last_received_at='2000-01-01T00:00:00+00:00' WHERE id=$1")
        .bind(&id)
        .execute(&s.repo.pool)
        .await
        .expect("age run");
    s.maintain().await.expect("timeout");
    let r = s.repo.report(&id).await.expect("report");
    assert_eq!(r["run"]["status"], "timed_out");
    assert_eq!(r["classification"]["accuracy"], Value::Null);
    assert_eq!(r["classification"]["incorrect"], Value::Null);
    assert_eq!(r["classification"]["samples"], 2);
    assert_eq!(r["embedded"]["inference_ms"]["mean"], 1.2);
    assert_eq!(
        r["embedded"]["thermal"]["average_chip_temperature_c"],
        Value::Null
    );
    let (_dir, empty) = fixture().await;
    send(&empty, MessageType::RunStart, "start", json!({})).await;
    send(
        &empty,
        MessageType::RunComplete,
        "end",
        json!({"status":"aborted"}),
    )
    .await;
    empty.maintain().await.expect("empty report");
    let id = run_id(&empty).await;
    assert_eq!(
        empty.repo.report(&id).await.expect("report")["embedded"]["inference_ms"]["mean"],
        Value::Null
    );
}

#[tokio::test]
async fn multiclass_missing_denominators_restart_and_conflicting_duplicate() {
    let (dir, s) = fixture().await;
    send(&s,MessageType::RunStart,"start",json!({"mode":"evaluation","model":{"class_names":["A","B","C","never","never"]},"configuration":{"sampling_period_ms":1.0}})).await;
    let pairs = [("A", "A"), ("A", "A"), ("A", "B"), ("B", "B"), ("C", "B")];
    for (i, (a, p)) in pairs.iter().enumerate() {
        send(&s,MessageType::InferenceResult,&format!("sample{i}"),json!({"actual_class":a,"predicted_class":p,"timing":{"inference_us":1000,"total_pipeline_us":1500}})).await;
    }
    send(&s,MessageType::MetricsSnapshot,"metrics",json!({"memory":{"free_heap_bytes":1000,"minimum_free_heap_bytes":500,"tensor_arena_bytes":200},"thermal":{"chip_temperature_c":42.0},"reliability":{"inference_errors":2}})).await;
    send(
        &s,
        MessageType::Event,
        "event",
        json!({"level":"WARN","message":"test"}),
    )
    .await;
    let (t, p) = payload(
        MessageType::InferenceResult,
        "sample0",
        json!({"actual_class":"changed"}),
    );
    assert!(s.ingest(&t, &p).await.is_err());
    let id = run_id(&s).await;
    let live = s.metrics(&id).await.expect("live metrics");
    assert_eq!(live["classification"]["accuracy"], 0.6);
    assert_eq!(live["class_metrics"].as_array().expect("classes").len(), 4);
    assert_eq!(live["class_metrics"][2]["precision"], Value::Null);
    assert_eq!(live["class_metrics"][2]["recall"], 0.0);
    assert_eq!(live["class_metrics"][3]["f1"], Value::Null);
    assert_eq!(live["classification"]["precision_weighted"], Value::Null);
    assert_eq!(live["embedded"]["deadlines"]["misses"], 5);
    assert_eq!(live["embedded"]["memory"]["minimum_free_heap_bytes"], 500.0);
    assert_eq!(
        live["embedded"]["thermal"]["maximum_chip_temperature_c"],
        42.0
    );
    send(
        &s,
        MessageType::RunComplete,
        "end",
        json!({"status":"failed","reliability":{"inference_errors":3}}),
    )
    .await;
    let storage = crate::storage::Storage::connect(&format!(
        "sqlite:{}?mode=rwc",
        dir.path().join("test.db").display()
    ))
    .await
    .expect("restart storage");
    let restarted = TinyMl::new(&storage, s.settings.clone());
    restarted.maintain().await.expect("resume report");
    let report = restarted.repo.report(&id).await.expect("report");
    assert_eq!(report["run"]["status"], "failed");
    assert_eq!(report["embedded"]["reliability"]["inference_errors"], 3);
    assert_eq!(
        restarted
            .repo
            .observations(&id, "events")
            .await
            .expect("events")
            .len(),
        1
    );
}

#[test]
fn hundred_thousand_observations_and_csv_safety() {
    let s = metrics::statistics((0..100000).map(f64::from).collect());
    assert_eq!(s.count, 100000);
    assert_eq!(s.median, Some(49999.5));
    assert_eq!(
        crate::routes::tinyml::csv_cell("=SUM(1,2)"),
        "\"'=SUM(1,2)\""
    );
    assert_eq!(crate::routes::tinyml::csv_cell("a\"b"), "\"a\"\"b\"");
}

#[tokio::test]
#[ignore = "Requires disposable PostgreSQL via TEST_DATABASE_URL"]
async fn postgres_tinyml_integration() {
    let storage = crate::storage::Storage::connect(
        &std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL"),
    )
    .await
    .expect("PostgreSQL");
    let device = format!("tinyml-test-{}", uuid::Uuid::new_v4());
    storage
        .upsert_device(&device, "Test", "esp32s3", "1", "127.0.0.1")
        .await
        .expect("register");
    let s = TinyMl::new(
        &storage,
        Settings {
            enabled: true,
            prefix: "tinyml/v1".into(),
            max_bytes: 65536,
            timeout_seconds: 300,
        },
    );
    for (kind, data) in [
        (MessageType::RunStart, json!({"mode":"evaluation"})),
        (
            MessageType::InferenceResult,
            json!({"sample_id":1,"actual_class":"A","predicted_class":"A","confidence":0.8,"timing":{"inference_us":1000}}),
        ),
        (
            MessageType::MetricsSnapshot,
            json!({"memory":{"free_heap_bytes":2000}}),
        ),
        (MessageType::RunComplete, json!({})),
    ] {
        let (_, bytes) = payload(kind, &uuid::Uuid::new_v4().to_string(), data);
        let mut e: protocol::Envelope = serde_json::from_slice(&bytes).expect("envelope");
        e.device_id = device.clone();
        let topic = kind.topic("tinyml/v1", &device, Some("run-1"));
        let bytes = serde_json::to_vec(&e).expect("serialize");
        assert!(s.ingest(&topic, &bytes).await.expect("ingest"));
        assert!(!s.ingest(&topic, &bytes).await.expect("duplicate"));
    }
    let runs = s
        .repo
        .runs(&models::Filters {
            device_id: Some(device.clone()),
            ..Default::default()
        })
        .await
        .expect("runs");
    let id = runs["items"][0]["id"].as_str().expect("id");
    s.repo.finalize(id).await.expect("report");
    assert_eq!(
        s.repo.report(id).await.expect("report")["classification"]["accuracy"],
        1.0
    );
    s.repo.overview().await.expect("SQL aggregate types");
    assert_eq!(
        s.repo
            .results(
                id,
                &models::Filters {
                    correct: Some(true),
                    min_confidence: Some(0.5),
                    ..Default::default()
                }
            )
            .await
            .expect("filters")["total"],
        1
    );
    // Only this test's UUID-scoped records, never truncate a shared table.
    for table in [
        "tinyml_results",
        "tinyml_metric_snapshots",
        "tinyml_events",
        "tinyml_reports",
        "tinyml_messages",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE run_key=$1"))
            .bind(id)
            .execute(storage.pool())
            .await
            .expect("cleanup");
    }
    sqlx::query("DELETE FROM tinyml_runs WHERE id=$1")
        .bind(id)
        .execute(storage.pool())
        .await
        .expect("cleanup");
    sqlx::query("DELETE FROM devices WHERE device_id=$1")
        .bind(&device)
        .execute(storage.pool())
        .await
        .expect("cleanup");
}
