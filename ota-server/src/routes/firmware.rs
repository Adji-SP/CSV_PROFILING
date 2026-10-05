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
    let firmware = state.storage.begin_firmware_deletion(&firmware_id).await?;
    let candidate = state.config.build_dir.join(&firmware.filename);
    // Missing files are normal on retry after interruption. Never follow an
    // untrusted path, even on this retry branch.
    if firmware.filename.is_empty()
        || firmware.filename.contains(['/', '\\', ':'])
        || matches!(firmware.filename.as_str(), "." | "..")
    {
        return Err(ApiError::internal("INVALID_FILENAME", "Firmware filename is not a safe basename"));
    }
    match tokio::fs::symlink_metadata(&candidate).await {
        Ok(metadata) => {
            // Never resolve an old image's symlink into another firmware file.
            if !metadata.file_type().is_file() {
                return Err(ApiError::conflict(
                    "UNSAFE_FIRMWARE_FILE",
                    "Firmware storage entry is not a regular file; it was not deleted",
                ));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(ApiError::conflict(
                        "UNSAFE_FIRMWARE_FILE",
                        "Firmware storage entry is a reparse point; it was not deleted",
                    ));
                }
            }
            ensure_existing_path_within(&state.config.build_dir, &candidate)?;
            // Unlink the exact entry, not its canonical target, even if a link
            // is swapped in after validation.
            match tokio::fs::remove_file(&candidate).await {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    state.storage.finish_firmware_deletion(&firmware_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::{io::Write, sync::Arc};

    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::DefaultBodyLimit,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    use zip::{ZipWriter, write::SimpleFileOptions};

    use crate::{
        config::Config,
        console::Console,
        services::{BuildCoordinator, LocalFirmwareBuilder},
        state::AppState,
        storage::Storage,
        tinyml::{Settings, TinyMl},
    };

    fn sample_project_zip() -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        for (path, contents) in [
            (
                "sample/Cargo.toml",
                "[package]\nname='sample'\nversion='0.1.0'\nedition='2021'\n[dependencies]\nfirmware-app-api='0.1'\n",
            ),
            (
                "sample/ota-app.toml",
                "schema_version=1\napplication_api_version=2\nentrypoint='src/main.rs'\n",
            ),
            ("sample/src/main.rs", "pub fn setup() {}\npub fn main() {}\n"),
        ] {
            zip.start_file(path, SimpleFileOptions::default())
                .expect("create ZIP entry");
            zip.write_all(contents.as_bytes()).expect("write ZIP entry");
        }
        zip.finish().expect("finish ZIP").into_inner()
    }

    fn multipart_body(zip: &[u8]) -> Vec<u8> {
        let mut body = b"--ota-test-boundary\r\nContent-Disposition: form-data; name=\"project\"; filename=\"sample.zip\"\r\nContent-Type: application/zip\r\n\r\n".to_vec();
        body.extend_from_slice(zip);
        body.extend_from_slice(b"\r\n--ota-test-boundary\r\nContent-Disposition: form-data; name=\"target\"\r\n\r\nesp32s3\r\n--ota-test-boundary\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n3.0.3\r\n--ota-test-boundary--\r\n");
        body
    }

    async fn test_app() -> (tempfile::TempDir, AppState, Router) {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().to_path_buf();
        let config = Arc::new(Config {
            console_only: false,
            bind_address: "127.0.0.1".into(),
            port: 0,
            public_base_url: "http://127.0.0.1:7000".into(),
            project_dir: root.join("projects"),
            build_dir: root.join("builds"),
            data_dir: root.clone(),
            runtime_template_dir: root.clone(),
            cargo_target_dir: root.clone(),
            esp_idf_tools_dir: None,
            python_path: None,
            max_upload_bytes: 1024 * 1024,
            max_extracted_bytes: 1024 * 1024,
            allowed_origins: vec!["http://localhost:5000".into()],
            rust_toolchain: None,
        });
        std::fs::create_dir_all(&config.project_dir).expect("projects directory");
        std::fs::create_dir_all(&config.build_dir).expect("builds directory");
        let storage = Storage::connect(&format!(
            "sqlite:{}?mode=rwc",
            root.join("test.db").display()
        ))
        .await
        .expect("temporary database");
        let builder = Arc::new(LocalFirmwareBuilder::new(
            config.build_dir.clone(),
            config.runtime_template_dir.clone(),
            config.cargo_target_dir.clone(),
            None,
            None,
            config.public_base_url.clone(),
            None,
        ));
        let coordinator = Arc::new(BuildCoordinator::new(
            storage.clone(),
            builder,
            config.project_dir.clone(),
            config.build_dir.clone(),
        ));
        let state = AppState {
            tinyml: TinyMl::new(
                &storage,
                Settings {
                    enabled: false,
                    prefix: "tinyml/v1".into(),
                    max_bytes: 65536,
                    timeout_seconds: 300,
                },
            ),
            console: Console::for_test("a".repeat(64)),
            config,
            storage,
            coordinator,
        };
        let app = crate::routes::router(state.clone()).layer(DefaultBodyLimit::max(1024 * 1024));
        (temp, state, app)
    }

    #[tokio::test]
    async fn upload_accepts_managed_zip_and_lists_project() {
        let (_temp, _state, app) = test_app().await;
        let request = Request::builder()
            .method("POST")
            .uri("/api/ota/projects")
            .header(
                header::CONTENT_TYPE,
                "multipart/form-data; boundary=ota-test-boundary",
            )
            .body(Body::from(multipart_body(&sample_project_zip())))
            .expect("upload request");
        let response = app.clone().oneshot(request).await.expect("upload response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("upload body");
        let uploaded: serde_json::Value = serde_json::from_slice(&body).expect("upload JSON");
        assert_eq!(uploaded["project_type"], "managed_application");
        assert_eq!(uploaded["version"], "3.0.3");

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/ota/projects")
                    .body(Body::empty())
                    .expect("list request"),
            )
            .await
            .expect("list response");
        assert_eq!(list.status(), StatusCode::OK);
        let body = to_bytes(list.into_body(), 1024 * 1024)
            .await
            .expect("list body");
        let projects: serde_json::Value = serde_json::from_slice(&body).expect("list JSON");
        assert_eq!(projects.as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn deletion_preserves_history_and_can_retry_missing_file() {
        use crate::models::{Firmware, Project};
        let (_temp, state, app) = test_app().await;
        let project = Project {
            id: "project".into(), name: "test".into(), target: "esp32s3".into(),
            version: "2.0.0".into(), path: "test".into(), status: "uploaded".into(),
            created_at: crate::storage::now(),
        };
        state.storage.insert_project(&project).await.expect("project");
        state.storage.insert_build("build", "project", "success").await.expect("build");
        let firmware = Firmware {
            id: "firmware".into(), project_id: project.id, build_id: "build".into(),
            project: "test".into(), target: "esp32s3".into(), version: "2.0.0".into(),
            filename: "test.bin".into(), path: "ignored/untrusted/path".into(),
            file_size: 4, sha256: "a".repeat(64), status: "ready".into(),
            created_at: crate::storage::now(),
        };
        state.storage.insert_firmware(&firmware).await.expect("firmware");
        let file = state.config.build_dir.join("test.bin");
        tokio::fs::write(&file, b"test").await.expect("test binary");
        state.storage.upsert_device("board", "Test", "esp32s3", "1.0.0", "127.0.0.1").await.expect("device");
        state.storage.create_deployment("deployment", "board", &firmware).await.expect("deploy");
        let delete = || Request::builder().method("DELETE").uri("/api/ota/firmware/firmware").body(Body::empty()).expect("request");
        assert_eq!(app.clone().oneshot(delete()).await.expect("protected").status(), StatusCode::CONFLICT);
        assert!(file.exists());
        state.storage.cancel_update("board").await.expect("cancel");
        assert_eq!(app.clone().oneshot(delete()).await.expect("delete").status(), StatusCode::NO_CONTENT);
        assert!(!file.exists());
        let history: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE firmware_id='firmware'").fetch_one(state.storage.pool()).await.expect("history");
        assert_eq!(history, 1);
        let download = Request::builder().uri("/api/ota/firmware/firmware/download").body(Body::empty()).expect("request");
        assert_eq!(app.clone().oneshot(download).await.expect("download").status(), StatusCode::NOT_FOUND);

        // Simulate a crash after unlink, before committing the tombstone.
        sqlx::query("UPDATE firmware SET status='deleting' WHERE id='firmware'").execute(state.storage.pool()).await.expect("crash fixture");
        assert_eq!(app.clone().oneshot(delete()).await.expect("retry").status(), StatusCode::NO_CONTENT);

        // Even a corrupted DB filename cannot delete outside firmware-builds.
        let outside = state.config.data_dir.join("keep.bin");
        tokio::fs::write(&outside, b"keep").await.expect("outside file");
        sqlx::query("UPDATE firmware SET status='ready',filename='../keep.bin' WHERE id='firmware'").execute(state.storage.pool()).await.expect("unsafe fixture");
        assert_eq!(app.oneshot(delete()).await.expect("unsafe delete").status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(outside.exists());
    }
}
