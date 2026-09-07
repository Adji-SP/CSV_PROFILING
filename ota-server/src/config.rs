use std::{
    env,
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
};

use crate::errors::{ApiError, ApiResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_address: String,
    pub port: u16,
    pub public_base_url: String,
    pub project_dir: PathBuf,
    pub build_dir: PathBuf,
    pub data_dir: PathBuf,
    pub runtime_template_dir: PathBuf,
    pub max_upload_bytes: usize,
    pub max_extracted_bytes: u64,
    pub allowed_origins: Vec<String>,
    pub rust_toolchain: Option<String>,
}

impl Config {
    pub fn load() -> ApiResult<Self> {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| ApiError::internal("CONFIG_ERROR", "Cannot locate workspace root"))?
            .to_path_buf();

        let _ = dotenvy::from_path(workspace_root.join(".env"));
        let _ = dotenvy::dotenv();

        let bind_address = env::var("OTA_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0".to_owned());
        let port = parse_env("OTA_PORT", 7000_u16)?;
        let max_upload_mb = parse_env("OTA_MAX_UPLOAD_MB", 50_usize)?;
        let max_extracted_mb = parse_env("OTA_MAX_EXTRACTED_MB", max_upload_mb.saturating_mul(10))?;

        let detected_host = detect_lan_ip().unwrap_or_else(|| "127.0.0.1".to_owned());
        let public_base_url = env::var("OTA_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| format!("http://{detected_host}:{port}"));
        if !(public_base_url.starts_with("http://") || public_base_url.starts_with("https://")) {
            return Err(ApiError::bad_request(
                "INVALID_CONFIG",
                "OTA_PUBLIC_BASE_URL must start with http:// or https://",
            ));
        }

        let project_dir = resolve_dir(
            &workspace_root,
            env::var("OTA_PROJECT_DIR").ok(),
            "firmware-projects",
        );
        let build_dir = resolve_dir(
            &workspace_root,
            env::var("OTA_BUILD_DIR").ok(),
            "firmware-builds",
        );
        let data_dir = resolve_dir(&workspace_root, env::var("OTA_DATA_DIR").ok(), "data");
        let runtime_template_dir = resolve_dir(
            &workspace_root,
            env::var("OTA_RUNTIME_DIR").ok(),
            "examples/esp32-rust-ota-client",
        );

        for directory in [&project_dir, &build_dir, &data_dir] {
            std::fs::create_dir_all(directory).map_err(|error| {
                ApiError::internal(
                    "DIRECTORY_CREATE_FAILED",
                    format!("Could not create {}: {error}", directory.display()),
                )
            })?;
        }

        let allowed_origins = env::var("OTA_ALLOWED_ORIGINS")
            .unwrap_or_else(|_| "http://localhost:5000,http://127.0.0.1:5000".to_owned())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let rust_toolchain = match env::var("OTA_RUST_TOOLCHAIN") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(value.trim().to_owned()),
            Err(_) => Some("esp".to_owned()),
        };

        Ok(Self {
            bind_address,
            port,
            public_base_url: public_base_url.trim_end_matches('/').to_owned(),
            project_dir,
            build_dir,
            data_dir,
            runtime_template_dir,
            max_upload_bytes: max_upload_mb.saturating_mul(1024 * 1024),
            max_extracted_bytes: (max_extracted_mb as u64).saturating_mul(1024 * 1024),
            allowed_origins,
            rust_toolchain,
        })
    }

    pub fn socket_addr(&self) -> ApiResult<SocketAddr> {
        format!("{}:{}", self.bind_address, self.port)
            .parse()
            .map_err(|error| {
                ApiError::internal(
                    "INVALID_BIND_ADDRESS",
                    format!("Invalid OTA bind address: {error}"),
                )
            })
    }

    pub fn database_url(&self) -> String {
        format!(
            "sqlite://{}",
            self.data_dir.join("ota.db").to_string_lossy()
        )
    }
}

fn parse_env<T>(name: &str, default: T) -> ApiResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value.parse().map_err(|error| {
            ApiError::internal(
                "INVALID_CONFIG",
                format!("{name} has an invalid value: {error}"),
            )
        }),
        Err(_) => Ok(default),
    }
}

fn resolve_dir(root: &Path, configured: Option<String>, default_name: &str) -> PathBuf {
    match configured.map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => root.join(path),
        None => root.join(default_name),
    }
}

pub fn detect_lan_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}
