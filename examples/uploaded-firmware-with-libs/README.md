# Managed firmware with libraries

This ZIP-ready example demonstrates that an uploaded application can use normal
Cargo dependencies. It uses `heapless`, `serde`, and `serde_json`; sensor,
display, MQTT, and other target-compatible crates can be added the same way.

Dependencies must compile for `xtensa-esp32s3-espidf`. Local path dependencies
must be included inside this project folder, and ESP-IDF core versions must stay
compatible with the stable runtime.

The example remains a normal function-oriented program in `src/main.rs`:
`setup` validates startup and returns the sample queue, while `main` contains the
long-running firmware loop.
