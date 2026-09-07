use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OtaStatus {
    Idle,
    Pending,
    Downloading,
    Verifying,
    Installing,
    Rebooting,
    Success,
    Failed,
}

impl OtaStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Installing => "installing",
            Self::Rebooting => "rebooting",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Device {
    pub device_id: String,
    pub name: String,
    pub chip: String,
    pub ip_address: String,
    pub current_version: String,
    pub desired_version: Option<String>,
    pub desired_firmware_id: Option<String>,
    pub status: String,
    pub ota_status: String,
    pub ota_progress: i64,
    pub ota_error: Option<String>,
    pub last_seen: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterDeviceRequest {
    pub device_id: String,
    pub name: String,
    pub chip: String,
    pub current_version: String,
    pub ip_address: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub current_version: String,
    #[serde(default = "default_online")]
    pub status: String,
    pub ip_address: String,
}

fn default_online() -> String {
    "online".to_owned()
}

#[derive(Debug, Deserialize)]
pub struct DeployRequest {
    pub firmware_id: String,
}

#[derive(Debug, Deserialize)]
pub struct OtaStatusRequest {
    pub status: OtaStatus,
    pub progress: Option<u8>,
    pub current_version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateResponse {
    pub update_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

impl UpdateResponse {
    pub const fn none() -> Self {
        Self {
            update_available: false,
            firmware_id: None,
            version: None,
            target: None,
            size: None,
            sha256: None,
            download_url: None,
        }
    }
}
