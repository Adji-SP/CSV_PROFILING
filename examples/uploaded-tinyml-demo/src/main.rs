//! Synthetic classification example: explicitly not a trained model benchmark.
//! Replace classify() with your model call; measurements cover the actual call below.
use anyhow::Result;
use firmware_app_api::{
    tinyml::{self, Complete, Inference, Memory, Model, Snapshot, Start, Telemetry, Timing},
    AppContext,
};
use serde_json::json;
use std::{
    thread,
    time::{Duration, Instant},
};

pub fn setup(_: &mut AppContext) -> Result<()> {
    Ok(())
}
fn classify(features: &[f32]) -> (&'static str, f64) {
    if features.iter().sum::<f32>() > 1.0 {
        ("high", 0.8)
    } else {
        ("low", 0.7)
    }
}
pub fn main(context: AppContext, _: ()) -> Result<()> {
    while !context.network.is_connected() {
        thread::sleep(Duration::from_millis(200));
    }
    // First verify registration on the dashboard; allow the OTA registration cycle to run.
    thread::sleep(Duration::from_secs(35));
    let mut telemetry = Telemetry::new(&context.device.device_id, &Telemetry::new_run_id())
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let start = Start {
        run_name: Some("Synthetic telemetry demonstration".into()),
        mode: Some("evaluation".into()),
        model: Some(Model {
            name: Some("demo-rule-classifier".into()),
            version: Some("1".into()),
            class_names: Some(vec!["low".into(), "high".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Err(e) = telemetry.start_run(start, json!({"synthetic":true})) {
        log::warn!("Start telemetry unavailable: {e:?}");
    }
    for sample in 0..100 {
        let features = [(sample % 10) as f32 / 10.0, 0.3];
        let before = Instant::now();
        let (predicted, confidence) = classify(&features);
        let elapsed = before.elapsed().as_micros() as f64;
        let system = tinyml::system_metrics();
        // Ground truth is the known synthetic rule, not a sensor label.
        let actual = if sample % 10 > 7 { "high" } else { "low" };
        let result = Inference {
            sample_id: json!(sample),
            actual_class: Some(actual.into()),
            predicted_class: Some(predicted.into()),
            confidence: Some(confidence),
            timing: Some(Timing {
                inference_us: Some(elapsed),
                ..Default::default()
            }),
            system: Some(system.clone()),
            ..Default::default()
        };
        let _ =
            telemetry.publish_result(result, json!({"feature_vector":features,"synthetic":true}));
        if sample % 10 == 0 {
            let _ = telemetry.publish_metrics(
                Snapshot {
                    samples_processed: Some(sample + 1),
                    memory: Some(Memory {
                        free_heap_bytes: system.free_heap_bytes,
                        minimum_free_heap_bytes: system.minimum_free_heap_bytes,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                json!({}),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = telemetry.complete_run(
        Complete {
            status: Some("completed".into()),
            samples_processed: Some(100),
            ..Default::default()
        },
        json!({"synthetic":true}),
    );
    loop {
        thread::sleep(Duration::from_secs(5));
        log::info!(
            "Demo complete; telemetry dropped={}",
            tinyml::dropped_messages()
        );
    }
}
