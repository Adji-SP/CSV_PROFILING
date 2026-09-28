# TinyML firmware example

This is an ordinary **function-based managed application**: `setup(context)` and
`main(context, state)` live in `src/main.rs`. No application class is required.
The existing runtime continues owning Wi-Fi, OTA and the single MQTT connection.

The example produces 100 **synthetic** classifications. The small threshold rule
and illustrative confidence values are not a trained-model benchmark. Timing and
heap measurements are read on the device; temperature is left unknown.

## Try it

1. Configure the root `.env` Wi-Fi, OTA address, `DEVICE_ID`, and existing
   `DEVICE_MQTT_*` TLS identity. Add TinyML broker ACLs as described in
   [the guide](../../docs/TINYML_GUIDE.md). Never package secrets in the ZIP.
2. Ensure this same device ID is registered in the dashboard. The demo waits for
   Wi-Fi and allows 35 seconds for the existing registration cycle; that delay
   is not a guarantee of registration if the OTA server is unreachable.
3. From the repository root, package only the application:

   ```powershell
   Compress-Archive -Path examples/uploaded-tinyml-demo/Cargo.toml,examples/uploaded-tinyml-demo/ota-app.toml,examples/uploaded-tinyml-demo/src -DestinationPath tinyml-demo.zip
   ```

   Linux/macOS: inside this folder, `zip -r ../../tinyml-demo.zip Cargo.toml ota-app.toml src`.
4. Upload the ZIP on **OTA Firmware**, select ESP32-S3, give it a new firmware
   version, build, and deploy through your existing OTA workflow. Initial boards
   still require the existing USB provisioning flow. This example does not flash
   or alter your partition table automatically.
5. Open **TinyML**, enter the viewer token, and choose the new run. After 100
   samples, a saved report should appear. Remote Console remains available.

## Integrate your real model

Keep `setup` and `main`. Replace `classify()` and its synthetic input with your
existing preprocessing/model/postprocessing. Call these helpers around that code:

```rust,ignore
use firmware_app_api::tinyml::{Telemetry, Start, Inference, Timing, Complete};
use serde_json::json;

let mut telemetry = Telemetry::new(&context.device.device_id, &Telemetry::new_run_id())?;
let _ = telemetry.start_run(Start { mode: Some("deployment".into()), ..Default::default() }, json!({}));
// Measure the actual call; do not substitute a guessed duration.
let _ = telemetry.publish_result(Inference {
    predicted_class: Some(prediction),
    actual_class: None, // supply a known label only for evaluation
    confidence: Some(confidence),
    timing: Some(Timing { inference_us: Some(measured_us), ..Default::default() }),
    ..Default::default()
}, json!({"your_future_field": your_value}));
let _ = telemetry.complete_run(Complete { status: Some("completed".into()), ..Default::default() }, json!({}));
```

`PublishError` is intentionally a small enum: handle/log it without unwinding the
inference loop (see the complete `main.rs` for conversion at initialization).
The shared API also exposes `publish_metrics`, `publish_event`, `system_metrics`,
and `dropped_messages`. Other application libraries remain normal Cargo dependencies.

Calls use a bounded eight-packet queue and a 4 KiB encoded packet limit, with
nonblocking `try_send`. They never wait for a broker. Disabled transport, oversized
packets or a full queue return an error and increment the telemetry-drop count.
The background worker retains a packet after immediate publish failure, and ESP-MQTT
handles QoS 1 retransmission with the original message ID. Its outbox is bounded.
The queue/outbox are RAM-only: power loss can lose pending messages. Retry control
messages from your own application state machine when appropriate; do not spin
or wait for the network inside a time-critical inference loop. Increase buffering
or add a durable spool only after measuring device memory needs.

The helper is in `examples/esp32-rust-ota-client/crates/application-api/src/tinyml.rs`.
Wire types are shared with the server in the adjacent `tinyml-protocol` crate.
The dashboard composes a generated copy of this application with those crates;
the uploaded source and existing user projects are not overwritten.
