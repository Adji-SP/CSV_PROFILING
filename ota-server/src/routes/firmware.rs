use std::path::Path as FilePath;

use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use chrono::Utc;
use serde::Serialize;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    errors::{ApiError, ApiResult},
    models::{BuildStatus, FirmwareResponse, LatestFirmwareQuery, Project},
    services::firmware::{
        ensure_existing_path_within, extract_project_zip, sanitize_component, validate_target,
        validate_version,
    },
    services::project::{APPLICATION_API_VERSION, ProjectKind, inspect_project},
    state::AppState,
};

#[derive(Serialize)]
pub struct ProjectUploadResponse {
    project_id: String,
    name: String,
    target: String,
    version: String,
    status: String,
    project_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    application_api_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<String>,
}

pub async fn upload_project(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<ProjectUploadResponse>)> {
    let mut archive = None;
    let mut original_filename = None;
    let mut target = None;
    let mut version = None;

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ApiError::bad_request("INVALID_MULTIPART", "Could not parse multipart upload")
            .with_details(error.to_string())
    })? {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "project" => {
                let filename = field.file_name().unwrap_or_default().to_owned();
                let is_zip = FilePath::new(&filename)
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("zip"));
                if !is_zip {
                    return Err(ApiError::unprocessable(
                        "INVALID_FILE_TYPE",
                        "Project upload must be a .zip file",
                    ));
                }
                let bytes = field.bytes().await.map_err(|error| {
                    ApiError::bad_request("UPLOAD_READ_FAILED", error.to_string())
                })?;
                if bytes.len() > state.config.max_upload_bytes {
                    return Err(ApiError::payload_too_large(state.config.max_upload_bytes));
                }
                archive = Some(bytes.to_vec());
                original_filename = Some(filename);
            }
            "target" => {
                target =
                    Some(field.text().await.map_err(|error| {
                        ApiError::bad_request("INVALID_TARGET", error.to_string())
                    })?);
            }
            "version" => {
                version = Some(field.text().await.map_err(|error| {
                    ApiError::bad_request("INVALID_VERSION", error.to_string())
                })?);
            }
            _ => {}
        }
    }

    let bytes = archive.ok_or_else(|| {
        ApiError::bad_request("PROJECT_REQUIRED", "Multipart field 'project' is required")
    })?;
    let target = target
        .unwrap_or_else(|| "esp32s3".to_owned())
        .to_lowercase();
    let version = version
        .ok_or_else(|| ApiError::bad_request("VERSION_REQUIRED", "Firmware version is required"))?;
    validate_target(&target)?;
    validate_version(&version)?;

    let filename = original_filename.unwrap_or_else(|| "firmware-project.zip".to_owned());
    let project_name = sanitize_component(
        FilePath::new(&filename)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("firmware-project"),
    );
    let project_id = Uuid::new_v4().to_string();
    let destination = state.config.project_dir.join(&project_id);
    let build_root =
        match extract_project_zip(bytes, destination.clone(), state.config.max_extracted_bytes)
            .await
        {
            Ok(path) => path,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&destination).await;
                return Err(error);
            }
        };
    ensure_existing_path_within(&state.config.project_dir, &build_root)?;
    let inspection = match inspect_project(&build_root) {
        Ok(inspection) => inspection,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&destination).await;
            let mut api_error = ApiError::unprocessable("INVALID_FIRMWARE_PROJECT", error.message);
            if let Some(details) = error.details {
                api_error = api_error.with_details(details);
            }
            return Err(api_error);
        }
    };

    let project = Project {
        id: project_id.clone(),
        name: project_name.clone(),
        target: target.clone(),
        version: version.clone(),
        path: build_root.to_string_lossy().into_owned(),
        status: BuildStatus::Uploaded.as_str().to_owned(),
        created_at: Utc::now().to_rfc3339(),
    };
    if let Err(error) = state.storage.insert_project(&project).await {
        let _ = tokio::fs::remove_dir_all(&destination).await;
        return Err(error);
    }

    Ok((
        StatusCode::CREATED,
        Json(ProjectUploadResponse {
            project_id,
            name: project_name,
            target,
            version,
            status: BuildStatus::Uploaded.as_str().to_owned(),
            project_type: inspection.kind.as_str().to_owned(),
            application_api_version: matches!(inspection.kind, ProjectKind::ManagedApplication)
                .then_some(APPLICATION_API_VERSION),
            entrypoint: inspection
                .entrypoint
                .map(|path| path.to_string_lossy().replace('\\', "/")),
        }),
    ))
}

pub async fn list_projects(State(state): State<AppState>) -> ApiResult<Json<Vec<Project>>> {
    Ok(Json(state.storage.list_projects().await?))
}

pub async fn list_firmware(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<FirmwareResponse>>> {
    let base = &state.config.public_base_url;
    Ok(Json(
        state
            .storage
            .list_firmware()
            .await?
            .into_iter()
            .map(|firmware| FirmwareResponse::new(firmware, base))
            .collect(),
    ))
}

pub async fn get_firmware(
    State(state): State<AppState>,
    Path(firmware_id): Path<String>,
) -> ApiResult<Json<FirmwareResponse>> {
    let firmware = state.storage.get_firmware(&firmware_id).await?;
    Ok(Json(FirmwareResponse::new(
        firmware,
        &state.config.public_base_url,
    )))
}

pub async fn latest_firmware(
    State(state): State<AppState>,
    Query(query): Query<LatestFirmwareQuery>,
) -> ApiResult<Json<FirmwareResponse>> {
    validate_target(&query.target)?;
    let firmware = state.storage.latest_firmware(&query.target).await?;
    Ok(Json(FirmwareResponse::new(
        firmware,
        &state.config.public_base_url,
    )))
}

pub async fn download_firmware(
    State(state): State<AppState>,
    Path(firmware_id): Path<String>,
) -> ApiResult<Response> {
    let firmware = state.storage.get_firmware(&firmware_id).await?;
    let candidate = state.config.build_dir.join(&firmware.filename);
    let safe_path = ensure_existing_path_within(&state.config.build_dir, &candidate)?;
    let file = tokio::fs::File::open(&safe_path).await?;
    let stream = ReaderStream::new(file);
    let disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        sanitize_component(&firmware.filename)
    ))
    .map_err(|error| ApiError::internal("INVALID_FILENAME", error.to_string()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, firmware.file_size)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header("X-Firmware-SHA256", firmware.sha256)
        .body(Body::from_stream(stream))
        .map_err(|error| ApiError::internal("RESPONSE_BUILD_FAILED", error.to_string()))
}

pub async fn delete_firmware(
    State(state): State<AppState>,
    Path(firmware_id): Path<String>,
) -> ApiResult<StatusCode> {
    let firmware = state.storage.get_firmware(&firmware_id).await?;
    if state.storage.firmware_reference_count(&firmware_id).await? > 0 {
        return Err(ApiError::conflict(
            "FIRMWARE_IN_USE",
            "Firmware is referenced by a device or deployment and cannot be deleted",
        ));
    }
    let candidate = state.config.build_dir.join(&firmware.filename);
    let safe_path = ensure_existing_path_within(&state.config.build_dir, &candidate)?;
    tokio::fs::remove_file(safe_path).await?;
    state.storage.delete_firmware(&firmware_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
