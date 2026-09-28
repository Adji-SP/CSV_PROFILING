# TinyML: from your firmware to a useful report

You do not need to move your model into the dashboard. Your ESP32 still runs
the application and inference. It sends **measurements**, not executable code,
to the existing MQTT broker. The Rust server saves them; the browser displays them.

```mermaid
flowchart LR
    A[ESP32 application: setup + main] -->|nonblocking queue| B[Existing MQTT TLS worker]
    B -->|QoS 1: tinyml/v1/...| C[Broker: device-specific ACL]
    C --> D[Rust: validate + deduplicate]
    D -->|atomic transaction| E[(Existing SQLite or PostgreSQL)]
    E --> F[Background report calculation]
    D -->|WebSocket summaries| G[TinyML dashboard]
    G -->|authenticated REST| E
    F --> E
```

CSV profiling still uses Flask and Pandas. OTA still builds and deploys firmware.
Remote Console still shows application logs. TinyML is a separate page, protocol,
set of tables, and WebSocket stream; telemetry never initiates a firmware action.

## 1. Get connected

1. Keep the existing `.env`, MQTT TLS credentials and SQL connection. Do not put
   credentials inside an uploaded ZIP. Back up your database before upgrading.
2. Add the four `TINYML_*` settings from `.env.example`. Defaults are enabled,
   prefix `tinyml/v1`, 65,536 bytes per message, and a 300-second run timeout.
3. Set `CONSOLE_VIEWER_TOKEN` to a random secret of at least 32 characters. The
   TinyML page uses the same operator token as Remote Console. It is not a device
   MQTT password. Browsers keep it only in memory, never in URLs or localStorage.
4. Extend broker permissions: the backend subscribes to `tinyml/v1/#`; each
   authenticated device may publish **only** `tinyml/v1/<its-device-id>/#`.
   Keep the existing `devices/+/logs` subscription/permissions as well.
5. Register the board with the existing OTA service, or use the existing device
   provisioning workflow. `device_id` must match the MQTT identity and envelope.
6. Restart the Rust service (`run-dev.bat` / `run-dev.sh`). Migration 002 adds
   TinyML tables; it does not replace CSV, devices, firmware or console storage.
7. Open **TinyML**, enter the viewer token, and press **Connect**.

No run appears merely because a USB cable is attached. The firmware must publish
TinyML messages. Existing firmware needs rebuilding with the new helper/runtime.
Follow [the demonstration firmware](../examples/uploaded-tinyml-demo/README.md)
to try the full flow before replacing its classifier with your own model.

## 2. What a run means

A run is one experiment or one bounded deployment session. Generate a new run ID
each time it starts, including after reboot. The wire identity is
`(device_id, run_id)`. REST responses also provide a server-generated UUID `id`;
use **that `id`** in `/api/tinyml/runs/{id}` URLs.

The normal lifecycle is `created → running → completing → completed`. Completion
can instead end in `failed`, `cancelled`, or `timed_out`. `aborted` is accepted as
an alias for `cancelled`. Data arriving before `run_start` creates a placeholder
run, so a small amount of reordering does not lose results. A start message can
fill in its metadata once. A new experiment must not reuse a closed run ID.

The timeout measures **server-received run activity**, not a device clock. Device
status messages do not extend it. Increase it for intentionally slow experiments,
or publish periodic run metrics. The report worker scans persisted pending runs
every two seconds. If calculation/storage fails, it retries; restarting the
server does not lose the pending report. Reports have a unique key per run.

Completion seals the run. Late results are rejected rather than silently changing
a saved report. Publish start, results, and completion from one ordered device
queue; wait until results are enqueued before completing. QoS 1 is not a guarantee
that packets dropped by firmware before enqueueing will reach the server.

## 3. Wire protocol v1.0

Publish UTF-8 JSON with QoS 1 and **retain=false**. Retained telemetry is ignored.
Required envelope fields are `schema_version`, `message_id`, `message_type`,
`device_id`, and `timestamp` (use `null` without a synchronized clock). All run
messages require `run_id`; status does not. Sequence is optional, nonnegative.
IDs contain 1–128 ASCII letters, digits, `.`, `_`, or `-`. The managed runtime's
existing MQTT device-ID rules are slightly narrower: use letters/digits/`_`/`-`.

| Message type | Topic |
|---|---|
| `run_start` | `tinyml/v1/{device}/run/start` |
| `inference_result`, `inference_batch` | `tinyml/v1/{device}/run/{run}/result` |
| `metrics_snapshot` | `tinyml/v1/{device}/run/{run}/metrics` |
| `event` | `tinyml/v1/{device}/run/{run}/event` |
| `run_complete` | `tinyml/v1/{device}/run/{run}/complete` |
| `status` | `tinyml/v1/{device}/status` |

Start example (values are illustrative, not claims about your device):

```json
{
  "schema_version": "1.0", "message_id": "start-unique-001",
  "message_type": "run_start", "device_id": "ESP32-S3-001",
  "run_id": "experiment-001", "sequence": 0, "timestamp": null,
  "data": {
    "run_name": "First evaluation", "mode": "evaluation",
    "model": {"name": "my-classifier", "version": "1", "model_format": "tflite",
      "quantization": "int8", "class_names": ["A", "B"], "model_size_bytes": 32768},
    "device": {"board": "esp32s3", "firmware_version": "1.0.1"},
    "configuration": {"expected_samples": 100, "sampling_period_ms": 20}
  },
  "other": {"experiment": {"operator_note": "baseline"}}
}
```

Result example:

```json
{
  "schema_version": "1.0", "message_id": "result-unique-001",
  "message_type": "inference_result", "device_id": "ESP32-S3-001",
  "run_id": "experiment-001", "sequence": 1, "timestamp": null,
  "data": {
    "sample_id": 1, "actual_class": "A", "predicted_class": "B", "confidence": 0.72,
    "timing": {"preprocessing_us": 200, "inference_us": 1200,
      "postprocessing_us": 100, "total_pipeline_us": 1500},
    "system": {"free_heap_bytes": 220000, "minimum_free_heap_bytes": 210000,
      "chip_temperature_c": null}
  },
  "other": {"features": [0.4, 0.7], "calibration": {"enabled": true}, "label_source": "test set"}
}
```

`actual_class` can be omitted in deployment mode. The server derives correctness
from actual and predicted labels; it does not trust an advisory `correct` flag.
Confidence is a ratio from 0 to 1. Class labels are strings. Timings are
nonnegative microseconds in individual results; reports display milliseconds.
`chip_temperature_c` means **MCU internal temperature**, not room temperature.
Never substitute an ambient sensor reading or a fabricated zero.

Snapshot data can contain:

```json
{
  "samples_processed": 50,
  "timing": {"average_inference_ms": 1.2, "maximum_inference_ms": 1.9},
  "memory": {"free_heap_bytes": 220000, "minimum_free_heap_bytes": 210000, "tensor_arena_bytes": 32768},
  "thermal": {"chip_temperature_c": null},
  "reliability": {"dropped_samples": 0, "inference_errors": 0, "processing_errors": 0,
    "reset_count": 0, "telemetry_dropped": 2},
  "runtime": {"uptime_ms": 45000}
}
```

Wrap it in the same envelope with type `metrics_snapshot`. Other typed data:

- Event: `{"level":"WARN","code":"INPUT_SKIPPED","message":"Input unavailable"}`.
- Status: `{"status":"online","firmware_version":"1.0.1","runtime":{"uptime_ms":45000}}`.
- Completion: `{"status":"completed","samples_processed":100,"reliability":{"inference_errors":0},"final_system_state":{"free_heap_bytes":220000}}`.
- Batch: `{"results":[{"sample_id":1,"predicted_class":"A","inference_us":1200,"other":{"x":1}}]}`,
  type `inference_batch`, 1–256 items. Each item becomes one result row. The
  envelope's message ID plus batch index is unique. Batch-level `other` stays in
  the raw message audit; each row uses its own `other`.

Typed measurements may be absent, `null`, or the string `"unknown"`. Other wrong
types are rejected. Unknown schema versions are rejected centrally; v1.0 is the
only currently supported wire version. Unknown fields remain in raw message JSON,
but put extensions under `other` for automatic display and graph discovery.

`other` accepts any JSON: numbers, strings, booleans, null, arrays, nested objects.
It is not flattened in storage, does not create columns, and never triggers a
command. Full result custom data is available in row details and CSV. Reports
contain bounded representative examples (256 JSON-pointer keys, depth 6, 2 KiB
per example) plus exact run/completion custom data, with a truncation flag.

### Deduplication and acknowledgment

Generate a unique `message_id` once per logical packet. A broker redelivery must
keep the same ID and payload. Identical duplicates do not add rows, counters, or
reports. Reusing an ID for different content is rejected. Sample IDs are not
deduplication keys: two measurements may legitimately refer to the same sample.

The backend acknowledges valid QoS 1 messages only after the database transaction
commits. Database failures retry without acknowledging. Malformed, unauthorized
by identity, or closed-run messages are logged and acknowledged to avoid an
infinite poison-message loop. Enforce broker ACLs: matching a claimed JSON ID to
its topic is not cryptographic proof of the publisher's identity.

## 4. Reading the dashboard

Connect, choose a device or status filter, then click a run. The page shows:

- Overview and live counters; bounded WebSocket summaries trigger coalesced refreshes.
- Paginated history (20/page), results (50/page), class/outcome/confidence/search
  filters, sorting and expandable complete row data.
- A signal plot for the latest 200 results. Numeric nested `other` values and
  arrays become selectable signals; missing temperature/heap do not become zeros.
- A provisional live report, refreshed at most every 15 seconds on incoming
  activity, or manually with **Refresh metrics**. Saved reports are immutable.
- Per-class metrics, confusion matrix, latency/memory/thermal/reliability,
  numeric findings, metadata and custom JSON. Large matrices expand as JSON.
- **Report JSON** after completion and **Results CSV** for all results, not just
  the visible page/filter. CSV quotes values and neutralizes spreadsheet formulas.

Reconnects request authoritative database state. A missed WebSocket event does
not mean a lost measurement. No browser polling occurs while disconnected.
Disconnect leaves already-loaded results on screen; clear/reload the page on a
shared computer. The token input is a password field, not persistent storage.

## 5. Metric definitions and honest limits

Only rows with both actual and predicted labels contribute to accuracy and the
confusion matrix. `accuracy = correct / labelled`. Deployment without labels has
null accuracy, incorrect count, precision, recall and F1—not zero.

For a class: precision = TP/predicted, recall = TP/support, F1 = 2TP/(support+predicted).
A zero denominator means null. Macro averages include **defined** class metrics
only. Weighted averages use ground-truth support; an undefined metric for a
supported class makes that weighted metric null. Declared and observed labels
are included, sorted, and deduplicated. More than 128 labels uses a sparse matrix
instead of allocating an unbounded square. Confidence means are separate for
all, correct and incorrect rows.

Exact quantiles sort finite observations and linearly interpolate index
`(n−1) × p`. No measurements means null; one measurement gives the same percentile
at every p. Latency statistics come from **per-inference rows**, never repeated
device averages. Device summaries are retained separately as `latest_device_snapshot`.
Reports stream database pages into background calculation; exact percentiles
still require O(n) numeric memory, so size the server for your run lengths.

Throughput is received result count / server-observed run duration. It is not an
estimate of CPU-only model throughput. Memory and thermal observations include
results, snapshots and completion, so reporting cadence affects their means.
Counters in snapshots/completion are cumulative; the report takes their maximum
rather than adding repeated counters. Missing counters remain null. The helper's
telemetry-drop counter is cumulative since boot, not automatically reset per run.

Flash and RAM percentages require explicit capacities. Free heap is not total
RAM. The report does not guess a board's flash, SRAM, tensor arena or model size.
Deadline analysis requires `sampling_period_ms` and measured pipeline times.
`real_time_capable` means only that **all observed pipelines met that period**;
it is not a scheduling or hard-real-time guarantee. Findings are deterministic
descriptions of measurements, not declarations that a model is “best”.

## 6. API and storage

All following GET endpoints require `Authorization: Bearer <CONSOLE_VIEWER_TOKEN>`:

| Path | Purpose |
|---|---|
| `/api/tinyml/status` | Configuration and overview counters |
| `/api/tinyml/devices` | Existing registered devices |
| `/api/tinyml/runs` | `page`, `page_size`, `device_id`, `status` |
| `/api/tinyml/runs/{id}` | Live run metadata/counters |
| `/api/tinyml/runs/{id}/results` | Paginated measured rows |
| `/api/tinyml/runs/{id}/metrics` | On-demand consistent report snapshot |
| `/api/tinyml/runs/{id}/report` | Persisted report, 404 until ready |
| `/api/tinyml/runs/{id}/report/json` | Download persisted report |
| `/api/tinyml/runs/{id}/results.csv` | Stream a consistent snapshot of all results |
| `/api/tinyml/runs/{id}/other-fields` | Custom report examples |
| `/api/tinyml/runs/{id}/snapshots` | Latest 500 snapshots |
| `/api/tinyml/runs/{id}/events` | Latest 500 run events |

Results support `actual_class`, `predicted_class`, `correct=true|false`,
`min_confidence`, `max_confidence`, `search`, `sort=time|sequence|sample|confidence|inference`,
`order=asc|desc`, and pagination capped at 200/page. Missing runs return404;
invalid limits/ranges return400. Search is case-insensitive SQL LIKE matching.

WebSocket `/api/tinyml/ws` checks `OTA_ALLOWED_ORIGINS`; the first frame must be
`{"token":"..."}` within five seconds. Events include `tinyml.connected`,
`tinyml.run.started`, `tinyml.inference`, `tinyml.metrics.updated`, `tinyml.event`,
`tinyml.run.completing`, `tinyml.run.completed`, `tinyml.report.ready`, and
`tinyml.resync`. This is an operator-wide stream, not per-user/tenant authorization.

Migration 002 creates `tinyml_runs`, `tinyml_messages`, `tinyml_results`,
`tinyml_metric_snapshots`, `tinyml_events`, `tinyml_reports`, and indexes. It
extends the existing SQLx storage/pool with no separate database. JSON is stored
as text for SQLite/PostgreSQL compatibility. SQLite uses WAL so report snapshots
can coexist with incoming writes. Terminal reports and source rows survive restart.
There is no automatic data-retention deletion: plan backups and retention before
continuous production use. Device status events are audited separately from OTA
heartbeat state; they do not mutate OTA deployment/version fields.

## 7. Azure deployment boundary

Keep the existing PostgreSQL backend and TLS MQTT setup; this feature does not
provision Azure resources or introduce Cosmos DB/Azure SQL Server drivers.
`DATABASE_URL` accepts SQLite or PostgreSQL, **not Microsoft SQL Server/T-SQL**.
For Azure Database for PostgreSQL, retain certificate validation (`sslmode=verify-full`)
and the existing network/private-endpoint policy. See [remote console setup](remote-console-azure.md)
for the base deployment configuration.

Extend your Azure Event Grid MQTT topic spaces/permission bindings to the TinyML
topics above. Give the backend subscribe rights and devices identity-scoped
publish rights. Provision certificates independently per board. Preserve broker
sessions for QoS 1 redelivery; the server uses a stable persistent MQTT session.
Only run **one MQTT ingestion instance per client identity**. Horizontal replicas
need distinct client IDs/shared-subscription architecture; in-process WebSocket
broadcasts do not currently fan out between replicas.

Put Axum behind HTTPS/WSS and include the exact dashboard origin in the whitelist.
Keep `OTA_CONSOLE_ONLY=true` for a hosted read-only monitoring instance: TinyML
read APIs and streaming remain available but trusted build/device-write APIs are
not exposed. Pre-register devices in the shared database through your trusted
provisioning service; this mode does not expose a new public registration API.
Do not expose the developer Cargo build service to untrusted users.

## 8. Tests and troubleshooting

From `ota-server`:

```powershell
cargo +stable-x86_64-pc-windows-gnu test
cargo +stable-x86_64-pc-windows-gnu check
```

On Linux/macOS use your normal host toolchain (`cargo test`, `cargo check`). The
tests exercise malformed/optional data, custom JSON, deduplication, run lifecycle,
timeouts, percentiles, classification, storage, authorization, WebSocket isolation,
filters and downloads. PostgreSQL tests are opt-in with a **disposable**
`TEST_DATABASE_URL`; do not point them at your production database.

```powershell
$env:TEST_DATABASE_URL='postgresql://user:password@localhost/tinyml_test'
cargo +stable-x86_64-pc-windows-gnu test postgres -- --ignored
```

Common checks:

- **No runs:** registered device? Correct MQTT ACL/prefix? Firmware actually
  rebuilt? TLS connected? Backend log says rejected/unregistered?
- **401:** use the server viewer token, not MQTT credentials. Token length >=32.
- **Live connection unavailable:** correct origin, HTTPS/WSS proxy upgrade, server reachable?
- **Timed out:** network gap or measurement interval exceeded the run timeout.
- **Missing accuracy:** no paired ground truth; this is expected for deployment.
- **Missing temperature/model size:** firmware did not measure/report it.
- **Fewer rows than samples processed:** compare telemetry-drop counters and broker
  logs. The bounded firmware queue trades delivery for uninterrupted inference.
- **Saved report unchanged:** terminal runs are sealed. Start a new run ID.

The ESP helper and demonstration can be compile-checked without flashing. Passing
host tests or an ESP compile check does not prove broker ACLs, Azure routing, Wi-Fi,
or physical-device delivery; validate those in your target environment.
