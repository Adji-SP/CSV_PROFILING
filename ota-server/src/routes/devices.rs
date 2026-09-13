use axum::{ Json, extract::{ Path, State, rejection::JsonRejection }, http::StatusCode };
use serde::Serialize;
use uuid::Uuid;

use crate::{
    errors::{ ApiError, ApiResult },
    models::{
        DeployRequest,
        Device,
        HeartbeatRequest,
        OtaStatus,
        OtaStatusRequest,
        RegisterDeviceRequest,
        UpdateResponse,
    },
    services::firmware::validate_target,
    state::AppState,
};

#[derive(Serialize)]
pub struct DeploymentResponse {
    deployment_id: String,
    device_id: String,
    firmware_id: String,
    desired_version: String,
    status: &'static str,
}

pub async fn register_device(
    State(state): State<AppState>,
    payload: Result<Json<RegisterDeviceRequest>, JsonRejection>
) -> ApiResult<(StatusCode, Json<Device>)> {
    let request = parse_json(payload)?;
    validate_device_id(&request.device_id)?;
    validate_short_text("name", &request.name, 128)?;
    validate_target(&request.chip.to_lowercase())?;
    validate_short_text("current_version", &request.current_version, 64)?;
    validate_short_text("ip_address", &request.ip_address, 64)?;
    let device = state.storage.upsert_device(
        &request.device_id,
        &request.name,
        &request.chip.to_lowercase(),
        &request.current_version,
        &request.ip_address
    ).await?;
    Ok((StatusCode::CREATED, Json(device)))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    payload: Result<Json<HeartbeatRequest>, JsonRejection>
) -> ApiResult<Json<Device>> {
    let request = parse_json(payload)?;
    validate_device_id(&device_id)?;
    validate_short_text("current_version", &request.current_version, 64)?;
    validate_short_text("ip_address", &request.ip_address, 64)?;
    let status = request.status.to_lowercase();
    if
        !matches!(
            status.as_str(),
            "online" | "offline" | "updating" | "rebooting" | "updated" | "failed" | "unknown"
        )
    {
        return Err(ApiError::unprocessable("INVALID_DEVICE_STATUS", "Unsupported device status"));
    }
    Ok(
        Json(
            state.storage.heartbeat_device(
                &device_id,
                &request.current_version,
                &status,
                &request.ip_address
            ).await?
        )
    )
}

pub async fn list_devices(State(state): State<AppState>) -> ApiResult<Json<Vec<Device>>> {
    Ok(Json(state.storage.list_devices().await?))
}

pub async fn deploy(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    payload: Result<Json<DeployRequest>, JsonRejection>
) -> ApiResult<(StatusCode, Json<DeploymentResponse>)> {
    let request = parse_json(payload)?;
    let device = state.storage.get_device(&device_id).await?;
    let firmware = state.storage.get_firmware(&request.firmware_id).await?;
    if firmware.status != "ready" {
        return Err(ApiError::conflict("FIRMWARE_NOT_READY", "Only ready firmware can be deployed"));
    }
    if device.chip != firmware.target {
        return Err(
            ApiError::conflict(
                "TARGET_MISMATCH",
                format!(
                    "Device chip '{}' does not match firmware target '{}'",
                    device.chip,
                    firmware.target
                )
            )
        );
    }
    let deployment_id = Uuid::new_v4().to_string();
    state.storage.create_deployment(&deployment_id, &device_id, &firmware).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(DeploymentResponse {
            deployment_id,
            device_id,
            firmware_id: firmware.id,
            desired_version: firmware.version,
            status: "pending",
        }),
    ))
}

pub async fn check_update(
    State(state): State<AppState>,
    Path(device_id): Path<String>
) -> ApiResult<Json<UpdateResponse>> {
    let device = state.storage.get_device(&device_id).await?;
    let Some(firmware_id) = device.desired_firmware_id else {
        return Ok(Json(UpdateResponse::none()));
    };
    let firmware = state.storage.get_firmware(&firmware_id).await?;
    if firmware.target != device.chip || firmware.version == device.current_version {
        return Ok(Json(UpdateResponse::none()));
    }
    Ok(
        Json(UpdateResponse {
            update_available: true,
            firmware_id: Some(firmware.id.clone()),
            version: Some(firmware.version),
            target: Some(firmware.target),
            size: Some(firmware.file_size),
            sha256: Some(firmware.sha256),
            download_url: Some(
                format!(
                    "{}/api/ota/firmware/{}/download",
                    state.config.public_base_url.trim_end_matches('/'),
                    firmware.id
                )
            ),
        })
    )
}

pub async fn ota_status(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    payload: Result<Json<OtaStatusRequest>, JsonRejection>
) -> ApiResult<Json<Device>> {
    let request = parse_json(payload)?;
    let progress = request.progress.unwrap_or(match request.status {
        OtaStatus::Success => 100,
        _ => 0,
    });
    if progress > 100 {
        return Err(
            ApiError::unprocessable("INVALID_PROGRESS", "OTA progress must be between 0 and 100")
        );
    }
    if matches!(request.status, OtaStatus::Failed) && request.error.is_none() {
        return Err(
            ApiError::unprocessable(
                "OTA_ERROR_REQUIRED",
                "A failed OTA status must include an error message"
            )
        );
    }
    if let Some(version) = request.current_version.as_deref() {
        validate_short_text("current_version", version, 64)?;
    }
    Ok(
        Json(
            state.storage.update_ota_status(
                &device_id,
                request.status.as_str(),
                i64::from(progress),
                request.current_version.as_deref(),
                request.error.as_deref()
            ).await?
        )
    )
}

fn validate_device_id(value: &str) -> ApiResult<()> {
    let valid =
        !value.is_empty() &&
        value.len() <= 128 &&
        value
            .chars()
            .all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            });
    if valid {
        Ok(())
    } else {
        Err(
            ApiError::unprocessable(
                "INVALID_DEVICE_ID",
                "device_id must use 1-128 letters, numbers, '.', '-', or '_'"
            )
        )
    }
}

fn validate_short_text(name: &str, value: &str, max: usize) -> ApiResult<()> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(
            ApiError::unprocessable(
                "INVALID_FIELD",
                format!("{name} must be between 1 and {max} printable characters")
            )
        )
    } else {
        Ok(())
    }
}

fn parse_json<T>(payload: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    payload
        .map(|Json(value)| value)
        .map_err(|error| {
            ApiError::bad_request("INVALID_JSON", "Request body must be valid JSON").with_details(
                error.body_text()
            )
        })
}
