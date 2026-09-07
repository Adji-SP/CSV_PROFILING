# Firmware examples

This directory separates the stable ESP32-S3 runtime from applications uploaded
through the dashboard:

- `esp32-rust-ota-client/` is the embedded runtime. Flash it over USB once.
- `uploaded-firmware-basic/` is a minimal managed application.
- `uploaded-firmware-with-libs/` shows that uploaded applications can use normal
  Cargo libraries when those libraries support `xtensa-esp32s3-espidf`.

## Create an upload ZIP

The ZIP root must contain the application's `Cargo.toml`, `ota-app.toml`, and
`src/main.rs`. The application exposes plain `setup` and `main` functions; it
does not need an `Application` struct or framework trait implementation. Do not
ZIP the parent `examples` directory.

PowerShell:

```powershell
Compress-Archive -Path examples\uploaded-firmware-basic\* -DestinationPath uploaded-firmware-basic.zip
```

Linux/macOS:

```sh
cd examples/uploaded-firmware-basic
zip -r ../../uploaded-firmware-basic.zip Cargo.toml ota-app.toml src README.md
```

Upload that ZIP on the **OTA Firmware** page. The server copies the stable
runtime, mounts the uploaded package as its `device-app` dependency, and builds
one complete firmware image. The uploaded source is not rewritten.

The `firmware-app-api = "0.1"` entry is a portable placeholder. During a managed
build, the server replaces it only in the generated workspace with a path to the
runtime's matching API crate.
