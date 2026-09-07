# Basic managed firmware application

This folder is a ZIP-ready application add-on for the dashboard. Keep
`Cargo.toml`, `ota-app.toml`, and `src/` at the archive root.

The OTA server replaces the `firmware-app-api = "0.1"` placeholder in its
generated build copy. It does not edit this source folder.

The program is function-based: `setup(&mut AppContext)` performs fallible startup
work and returns application state, then `main(AppContext, state)` runs the normal
firmware loop. No application class or trait implementation is required.
