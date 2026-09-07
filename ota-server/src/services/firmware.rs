use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;
use zip::ZipArchive;

use crate::{
    errors::{ApiError, ApiResult},
    models::{BuildStatus, Firmware, Project},
    services::{
        builder::{BuildArtifact, BuildRequest, FirmwareBuilder},
        hashing::sha256_file,
    },
    storage::Storage,
};

pub struct BuildCoordinator {
    storage: Storage,
    builder: Arc<dyn FirmwareBuilder>,
    project_root: PathBuf,
    build_root: PathBuf,
}

impl BuildCoordinator {
    pub fn new(
        storage: Storage,
        builder: Arc<dyn FirmwareBuilder>,
        project_root: PathBuf,
        build_root: PathBuf,
    ) -> Self {
        Self {
            storage,
            builder,
            project_root,
            build_root,
        }
    }

    pub fn start(self: &Arc<Self>, project: Project, build_id: String) {
        let coordinator = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = coordinator.run(project, &build_id).await {
                tracing::error!(build_id, %error, "build coordinator failed");
            }
        });
    }

    async fn run(&self, project: Project, build_id: &str) -> ApiResult<()> {
        self.storage.start_build(build_id).await?;
        self.storage
            .set_project_status(&project.id, BuildStatus::Building.as_str())
            .await?;
        self.storage
            .append_build_log(build_id, "Build started")
            .await?;

        let project_path = PathBuf::from(&project.path);
        if let Err(error) = ensure_existing_path_within(&self.project_root, &project_path) {
            self.fail_build(
                build_id,
                &project.id,
                None,
                &error.message,
                error.details.as_deref(),
            )
            .await?;
            return Ok(());
        }

        let (log_tx, mut log_rx) = mpsc::unbounded_channel::<String>();
        let log_storage = self.storage.clone();
        let log_build_id = build_id.to_owned();
        let log_task = tokio::spawn(async move {
            while let Some(line) = log_rx.recv().await {
                if let Err(error) = log_storage.append_build_log(&log_build_id, &line).await {
                    tracing::error!(build_id = log_build_id, %error, "could not persist build log");
                }
            }
        });

        let result = self
            .builder
            .build(
                BuildRequest {
                    build_id: build_id.to_owned(),
                    project_name: project.name.clone(),
                    project_dir: project_path,
                    target: project.target.clone(),
                    version: project.version.clone(),
                },
                log_tx,
            )
            .await;
        let _ = log_task.await;

        match result {
            Ok(artifact) => {
                if let Err(error) = self.finalize_success(&project, build_id, artifact).await {
                    self.fail_build(
                        build_id,
                        &project.id,
                        None,
                        &error.message,
                        error.details.as_deref(),
                    )
                    .await?;
                }
            }
            Err(error) => {
                self.fail_build(
                    build_id,
                    &project.id,
                    error.exit_code,
                    &error.message,
                    error.details.as_deref(),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn finalize_success(
        &self,
        project: &Project,
        build_id: &str,
        artifact: BuildArtifact,
    ) -> ApiResult<()> {
        ensure_existing_path_within(&self.build_root, &artifact.path)?;
        let metadata = tokio::fs::metadata(&artifact.path).await?;
        let sha256 = sha256_file(&artifact.path).await?;
        self.storage
            .append_build_log(build_id, &format!("SHA-256: {sha256}"))
            .await?;

        let firmware = Firmware {
            id: Uuid::new_v4().to_string(),
            project_id: project.id.clone(),
            build_id: build_id.to_owned(),
            project: project.name.clone(),
            target: project.target.clone(),
            version: project.version.clone(),
            filename: artifact.filename,
            path: artifact.path.to_string_lossy().into_owned(),
            file_size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            sha256,
            status: "ready".to_owned(),
            created_at: Utc::now().to_rfc3339(),
        };
        self.storage.insert_firmware(&firmware).await?;
        self.storage
            .append_build_log(
                build_id,
                "Build successful. Firmware is ready for OTA deployment.",
            )
            .await?;
        self.storage
            .finish_build(
                build_id,
                BuildStatus::Success.as_str(),
                artifact.exit_code,
                None,
            )
            .await?;
        self.storage
            .set_project_status(&project.id, BuildStatus::Success.as_str())
            .await?;
        Ok(())
    }

    async fn fail_build(
        &self,
        build_id: &str,
        project_id: &str,
        exit_code: Option<i32>,
        message: &str,
        details: Option<&str>,
    ) -> ApiResult<()> {
        self.storage
            .append_build_log(build_id, &format!("BUILD FAILED: {message}"))
            .await?;
        if let Some(details) = details {
            self.storage.append_build_log(build_id, details).await?;
        }
        self.storage
            .finish_build(
                build_id,
                BuildStatus::Failed.as_str(),
                exit_code,
                Some(message),
            )
            .await?;
        self.storage
            .set_project_status(project_id, BuildStatus::Failed.as_str())
            .await?;
        Ok(())
    }
}

pub async fn extract_project_zip(
    archive_bytes: Vec<u8>,
    destination: PathBuf,
    max_extracted_bytes: u64,
) -> ApiResult<PathBuf> {
    tokio::task::spawn_blocking(move || {
        extract_project_zip_blocking(archive_bytes, &destination, max_extracted_bytes)
    })
    .await
    .map_err(|error| {
        ApiError::internal("ZIP_WORKER_FAILED", "ZIP extraction worker failed")
            .with_details(error.to_string())
    })?
}

fn extract_project_zip_blocking(
    archive_bytes: Vec<u8>,
    destination: &Path,
    max_extracted_bytes: u64,
) -> ApiResult<PathBuf> {
    fs::create_dir_all(destination)?;
    let cursor = Cursor::new(archive_bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|error| {
        ApiError::unprocessable(
            "INVALID_ZIP",
            "The uploaded file is not a valid ZIP archive",
        )
        .with_details(error.to_string())
    })?;

    if archive.len() > 10_000 {
        return Err(ApiError::unprocessable(
            "ZIP_TOO_MANY_ENTRIES",
            "ZIP archive contains too many entries",
        ));
    }

    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| ApiError::unprocessable("INVALID_ZIP_ENTRY", error.to_string()))?;
        let relative = entry.enclosed_name().ok_or_else(|| {
            ApiError::unprocessable(
                "UNSAFE_ZIP_PATH",
                format!("Unsafe ZIP entry rejected: {}", entry.name()),
            )
        })?;
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(ApiError::unprocessable(
                "UNSAFE_ZIP_PATH",
                format!("Unsafe ZIP entry rejected: {}", entry.name()),
            ));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(ApiError::unprocessable(
                "ZIP_SYMLINK_REJECTED",
                "Symbolic links are not allowed in uploaded projects",
            ));
        }

        extracted_bytes = extracted_bytes.saturating_add(entry.size());
        if extracted_bytes > max_extracted_bytes {
            return Err(ApiError::payload_too_large(
                usize::try_from(max_extracted_bytes).unwrap_or(usize::MAX),
            ));
        }

        let output_path = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output_path)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&output_path)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = entry
                .read(&mut buffer)
                .map_err(|error| ApiError::unprocessable("ZIP_READ_FAILED", error.to_string()))?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
    }

    find_manifest_root(destination)
}

fn find_manifest_root(destination: &Path) -> ApiResult<PathBuf> {
    let mut stack = vec![destination.to_path_buf()];
    let mut manifests = Vec::new();
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                let name = entry.file_name();
                if name != ".git" && name != "target" && name != ".ota-target" {
                    stack.push(path);
                }
            } else if entry.file_name() == "Cargo.toml" {
                manifests.push(path);
            }
        }
    }
    manifests.sort_by_key(|path| path.components().count());
    let manifest = manifests.first().ok_or_else(|| {
        ApiError::unprocessable(
            "CARGO_MANIFEST_MISSING",
            "The ZIP archive does not contain a Cargo.toml",
        )
    })?;
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| ApiError::unprocessable("INVALID_PROJECT", "Invalid Cargo project layout"))
}

pub fn sanitize_component(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = cleaned.trim_matches(['.', '_', '-']);
    if trimmed.is_empty() {
        "firmware".to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

pub fn validate_version(value: &str) -> ApiResult<()> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
        });
    if valid {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "INVALID_VERSION",
            "Version must be 1-64 characters using letters, numbers, '.', '-', '_', or '+'",
        ))
    }
}

pub fn validate_target(value: &str) -> ApiResult<()> {
    match value {
        "esp32s3" => Ok(()),
        _ => Err(ApiError::unprocessable(
            "UNSUPPORTED_TARGET",
            "Only ESP32-S3 (esp32s3 / xtensa-esp32s3-espidf) is currently supported",
        )),
    }
}

pub fn ensure_existing_path_within(root: &Path, candidate: &Path) -> ApiResult<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| ApiError::internal("PATH_VALIDATION_FAILED", error.to_string()))?;
    let canonical_candidate = candidate.canonicalize().map_err(|error| {
        ApiError::not_found("The requested file or directory does not exist")
            .with_details(error.to_string())
    })?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(ApiError::bad_request(
            "UNSAFE_PATH",
            "Path is outside the configured OTA storage directory",
        ));
    }
    Ok(canonical_candidate)
}

#[cfg(test)]
mod tests {
    use super::{sanitize_component, validate_version};

    #[test]
    fn sanitizes_artifact_components() {
        assert_eq!(
            sanitize_component("sensor project/../../bad"),
            "sensor_project_.._.._bad"
        );
    }

    #[test]
    fn rejects_path_like_versions() {
        assert!(validate_version("1.2.0").is_ok());
        assert!(validate_version("../../firmware").is_err());
        assert!(validate_version("").is_err());
    }
}
