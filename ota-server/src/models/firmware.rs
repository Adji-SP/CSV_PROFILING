use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildStatus {
    Uploaded,
    Queued,
    Building,
    Success,
    Failed,
}

impl BuildStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Uploaded => "uploaded",
            Self::Queued => "queued",
            Self::Building => "building",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub target: String,
    pub version: String,
    #[serde(skip_serializing)]
    pub path: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Build {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub target: String,
    pub version: String,
    pub status: String,
    #[serde(skip_serializing)]
    pub logs_text: String,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResponse {
    pub build_id: String,
    pub project_id: String,
    pub project_name: String,
    pub target: String,
    pub version: String,
    pub status: String,
    pub logs: Vec<String>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

impl From<Build> for BuildResponse {
    fn from(value: Build) -> Self {
        let hint = if value.logs_text.contains("link.exe` not found")
            || value.logs_text.contains("link.exe not found")
        {
            Some(
                "Install Visual Studio Build Tools with the Desktop development with C++ workload, then restart the OTA server terminal."
                    .to_owned(),
            )
        } else {
            value.error.as_ref().and_then(|error| {
                (error.contains("not installed")
                    || error.contains("not available on PATH")
                    || error.contains("toolchain check failed"))
                .then(|| {
                    "Install the ESP Rust toolchain and espflash, then ensure cargo, rustc, and espflash are on PATH.".to_owned()
                })
            })
        };
        let logs = value.logs_text.lines().map(ToOwned::to_owned).collect();
        Self {
            build_id: value.id,
            project_id: value.project_id,
            project_name: value.project_name,
            target: value.target,
            version: value.version,
            status: value.status,
            logs,
            exit_code: value.exit_code,
            error: value.error,
            hint,
            started_at: value.started_at,
            finished_at: value.finished_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Firmware {
    pub id: String,
    pub project_id: String,
    pub build_id: String,
    pub project: String,
    pub target: String,
    pub version: String,
    pub filename: String,
    #[serde(skip_serializing)]
    pub path: String,
    pub file_size: i64,
    pub sha256: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FirmwareResponse {
    pub firmware_id: String,
    pub project_id: String,
    pub build_id: String,
    pub project: String,
    pub target: String,
    pub version: String,
    pub filename: String,
    pub size: i64,
    pub sha256: String,
    pub status: String,
    pub created_at: String,
    pub download_url: String,
}

impl FirmwareResponse {
    pub fn new(value: Firmware, public_base_url: &str) -> Self {
        let download_url = format!(
            "{}/api/ota/firmware/{}/download",
            public_base_url.trim_end_matches('/'),
            value.id
        );
        Self {
            firmware_id: value.id,
            project_id: value.project_id,
            build_id: value.build_id,
            project: value.project,
            target: value.target,
            version: value.version,
            filename: value.filename,
            size: value.file_size,
            sha256: value.sha256,
            status: value.status,
            created_at: value.created_at,
            download_url,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LatestFirmwareQuery {
    pub target: String,
}
