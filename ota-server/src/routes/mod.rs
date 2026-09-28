pub mod builds;
pub mod devices;
pub mod firmware;
pub mod tinyml;

use axum::{
    Json, Router,
    routing::{get, post},
};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
struct StatusResponse {
    service: &'static str,
    language: &'static str,
    status: &'static str,
    version: &'static str,
    public_base_url: String,
    supported_targets: [&'static str; 1],
}

pub fn router(state: AppState) -> Router {
    let console_routes = Router::new()
        .merge(tinyml::router(state.clone()))
        .route("/api/ota/status", get(status))
        .route("/api/ota/console/ws", get(crate::console::stream))
        .route("/api/ota/console/devices", get(crate::console::devices));
    if state.config.console_only {
        return console_routes.fallback(not_found).with_state(state);
    }
    console_routes
        .route(
            "/api/ota/projects",
            post(firmware::upload_project).get(firmware::list_projects),
        )
        .route(
            "/api/ota/projects/{project_id}/build",
            post(builds::start_build),
        )
        .route("/api/ota/builds", get(builds::list_builds))
        .route("/api/ota/builds/{build_id}", get(builds::get_build))
        .route("/api/ota/firmware", get(firmware::list_firmware))
        .route("/api/ota/firmware/latest", get(firmware::latest_firmware))
        .route(
            "/api/ota/firmware/{firmware_id}",
            get(firmware::get_firmware).delete(firmware::delete_firmware),
        )
        .route(
            "/api/ota/firmware/{firmware_id}/download",
            get(firmware::download_firmware),
        )
        .route("/api/ota/devices", get(devices::list_devices))
        .route("/api/ota/devices/register", post(devices::register_device))
        .route(
            "/api/ota/devices/{device_id}/heartbeat",
            post(devices::heartbeat),
        )
        .route("/api/ota/devices/{device_id}/deploy", post(devices::deploy))
        .route(
            "/api/ota/devices/{device_id}/update",
            get(devices::check_update),
        )
        .route(
            "/api/ota/devices/{device_id}/ota-status",
            post(devices::ota_status),
        )
        .fallback(not_found)
        .with_state(state)
}

async fn status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<StatusResponse> {
    Json(StatusResponse {
        service: "CSV_PROFILING OTA Server",
        language: "rust",
        status: "online",
        version: env!("CARGO_PKG_VERSION"),
        public_base_url: state.config.public_base_url.clone(),
        supported_targets: ["esp32s3"],
    })
}

async fn not_found() -> crate::errors::ApiError {
    crate::errors::ApiError::not_found("OTA API route not found")
}
