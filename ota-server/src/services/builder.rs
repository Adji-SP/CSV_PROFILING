use std::{
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
};

use async_trait::async_trait;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::mpsc::UnboundedSender,
};

use crate::services::firmware::sanitize_component;
use crate::services::project::{ProjectKind, compose_managed_project, inspect_project};

#[derive(Debug, Clone)]
pub struct BuildRequest {
    pub build_id: String,
    pub project_name: String,
    pub project_dir: PathBuf,
    pub target: String,
    pub version: String,
}

#[derive(Debug)]
pub struct BuildArtifact {
    pub path: PathBuf,
    pub filename: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BuildFailure {
    pub message: String,
    pub details: Option<String>,
    pub exit_code: Option<i32>,
}

impl BuildFailure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: None,
            exit_code: None,
        }
    }

    fn command_failed(name: &str, status: ExitStatus) -> Self {
        let exit_code = status.code();
        Self {
            message: format!("{name} failed"),
            details: Some(format!(
                "Process exited with code {}",
                exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
            )),
            exit_code,
        }
    }
}

#[async_trait]
pub trait FirmwareBuilder: Send + Sync {
    async fn build(
        &self,
        request: BuildRequest,
        logs: UnboundedSender<String>,
    ) -> Result<BuildArtifact, BuildFailure>;
}

#[derive(Debug, Clone)]
pub struct LocalFirmwareBuilder {
    build_dir: PathBuf,
    runtime_template_dir: PathBuf,
    public_base_url: String,
    rust_toolchain: Option<String>,
}

impl LocalFirmwareBuilder {
    pub fn new(
        build_dir: PathBuf,
        runtime_template_dir: PathBuf,
        public_base_url: String,
        rust_toolchain: Option<String>,
    ) -> Self {
        Self {
            build_dir,
            runtime_template_dir,
            public_base_url,
            rust_toolchain,
        }
    }

    async fn verify_tool(
        &self,
        name: &str,
        logs: &UnboundedSender<String>,
    ) -> Result<(), BuildFailure> {
        let mut command = Command::new(name);
        command.arg("--version");
        self.apply_toolchain(&mut command);
        let output = command.output().await.map_err(|error| BuildFailure {
            message: format!("{name} is not installed or not available on PATH"),
            details: Some(error.to_string()),
            exit_code: None,
        })?;
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(BuildFailure {
                message: format!("{name} is installed but its toolchain check failed"),
                details: (!details.is_empty()).then_some(details),
                exit_code: output.status.code(),
            });
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let _ = logs.send(format!("Tooling: {version}"));
        Ok(())
    }

    fn apply_toolchain(&self, command: &mut Command) {
        if let Some(toolchain) = self.rust_toolchain.as_deref() {
            if !toolchain.is_empty() {
                command.env("RUSTUP_TOOLCHAIN", toolchain);
            }
        }
    }

    fn target_spec(target: &str) -> Result<TargetSpec, BuildFailure> {
        match target {
            "esp32s3" => Ok(TargetSpec {
                rust_target: "xtensa-esp32s3-espidf",
                espflash_chip: "esp32s3",
            }),
            _ => Err(BuildFailure::new(format!(
                "Unsupported firmware target: {target}"
            ))),
        }
    }
}

struct TargetSpec {
    rust_target: &'static str,
    espflash_chip: &'static str,
}

#[async_trait]
impl FirmwareBuilder for LocalFirmwareBuilder {
    async fn build(
        &self,
        request: BuildRequest,
        logs: UnboundedSender<String>,
    ) -> Result<BuildArtifact, BuildFailure> {
        let spec = Self::target_spec(&request.target)?;
        for tool in ["cargo", "rustc", "espflash"] {
            self.verify_tool(tool, &logs).await?;
        }

        let inspection = inspect_project(&request.project_dir).map_err(|error| BuildFailure {
            message: error.message,
            details: error.details,
            exit_code: None,
        })?;
        let build_source = match inspection.kind {
            ProjectKind::Standalone => {
                let _ = logs.send(
                    "Project mode: standalone firmware (the project must retain its own OTA client)"
                        .to_owned(),
                );
                request.project_dir.clone()
            }
            ProjectKind::ManagedApplication => {
                let package_name = inspection.package_name.ok_or_else(|| {
                    BuildFailure::new("Managed application package name is missing")
                })?;
                let entrypoint = inspection.entrypoint.ok_or_else(|| {
                    BuildFailure::new("Managed application entrypoint is missing")
                })?;
                let _ = logs.send(format!(
                    "Function application entrypoint: {} (API v2)",
                    entrypoint.display()
                ));
                let destination = request.project_dir.join(".ota-managed-runtime");
                let runtime_template = self.runtime_template_dir.clone();
                let uploaded_project = request.project_dir.clone();
                let destination_for_worker = destination.clone();
                let package_for_worker = package_name.clone();
                let _ = logs.send(format!(
                    "Project mode: managed application ({package_name})"
                ));
                let _ = logs.send(format!(
                    "Composing with stable runtime: {}",
                    runtime_template.display()
                ));
                tokio::task::spawn_blocking(move || {
                    compose_managed_project(
                        &runtime_template,
                        &uploaded_project,
                        &destination_for_worker,
                        &package_for_worker,
                        &entrypoint,
                    )
                })
                .await
                .map_err(|error| BuildFailure {
                    message: "Managed build workspace task failed".to_owned(),
                    details: Some(error.to_string()),
                    exit_code: None,
                })?
                .map_err(|error| BuildFailure {
                    message: error.message,
                    details: error.details,
                    exit_code: None,
                })?;
                destination
            }
        };

        let target_dir = request.project_dir.join(".ota-target");
        let _ = logs.send(format!(
            "$ cargo build --release --target {}",
            spec.rust_target
        ));
        let mut cargo = Command::new("cargo");
        cargo
            .arg("build")
            .arg("--release")
            .arg("--target")
            .arg(spec.rust_target)
            .current_dir(&build_source)
            .env("CARGO_TARGET_DIR", &target_dir)
            .env("FIRMWARE_VERSION", &request.version)
            .env("OTA_SERVER_URL", &self.public_base_url);
        self.apply_toolchain(&mut cargo);
        let cargo_status = run_logged(cargo, logs.clone()).await?;
        if !cargo_status.success() {
            return Err(BuildFailure::command_failed("Cargo build", cargo_status));
        }

        let release_dir = target_dir.join(spec.rust_target).join("release");
        let elf_path = locate_elf(&release_dir).await?;
        let _ = logs.send(format!("ELF: {}", elf_path.display()));

        let short_id = request.build_id.chars().take(8).collect::<String>();
        let filename = format!(
            "firmware-{}-v{}-{}.bin",
            sanitize_component(&request.project_name),
            sanitize_component(&request.version),
            short_id
        );
        let output_path = self.build_dir.join(&filename);
        let _ = logs.send("Generating ESP-IDF OTA application image...".to_owned());
        let _ = logs.send(format!(
            "$ espflash save-image --chip {} --format esp-idf <ELF> {}",
            spec.espflash_chip, filename
        ));

        let mut espflash = Command::new("espflash");
        espflash
            .arg("save-image")
            .arg("--chip")
            .arg(spec.espflash_chip)
            .arg("--format")
            .arg("esp-idf")
            .arg(&elf_path)
            .arg(&output_path)
            .current_dir(&build_source);
        let image_status = run_logged(espflash, logs.clone()).await?;
        if !image_status.success() {
            return Err(BuildFailure::command_failed(
                "espflash save-image",
                image_status,
            ));
        }

        match tokio::fs::metadata(&output_path).await {
            Ok(metadata) if metadata.len() > 0 => Ok(BuildArtifact {
                path: output_path,
                filename,
                exit_code: image_status.code(),
            }),
            Ok(_) => Err(BuildFailure::new(
                "espflash created an empty firmware image",
            )),
            Err(error) => Err(BuildFailure {
                message: "espflash did not create the expected OTA image".to_owned(),
                details: Some(error.to_string()),
                exit_code: image_status.code(),
            }),
        }
    }
}

async fn run_logged(
    mut command: Command,
    logs: UnboundedSender<String>,
) -> Result<ExitStatus, BuildFailure> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| BuildFailure {
        message: "Could not start build process".to_owned(),
        details: Some(error.to_string()),
        exit_code: None,
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BuildFailure::new("Could not capture build stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| BuildFailure::new("Could not capture build stderr"))?;

    let stdout_task = tokio::spawn(forward_lines(stdout, logs.clone()));
    let stderr_task = tokio::spawn(forward_lines(stderr, logs));
    let status = child.wait().await.map_err(|error| BuildFailure {
        message: "Could not wait for build process".to_owned(),
        details: Some(error.to_string()),
        exit_code: None,
    })?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    Ok(status)
}

async fn forward_lines<R>(reader: R, logs: UnboundedSender<String>)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let _ = logs.send(line);
            }
            Ok(None) => break,
            Err(error) => {
                let _ = logs.send(format!("Log stream error: {error}"));
                break;
            }
        }
    }
}

async fn locate_elf(release_dir: &Path) -> Result<PathBuf, BuildFailure> {
    let mut entries = tokio::fs::read_dir(release_dir)
        .await
        .map_err(|error| BuildFailure {
            message: "Cargo succeeded but the target release directory was not found".to_owned(),
            details: Some(error.to_string()),
            exit_code: Some(0),
        })?;
    let mut candidates = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|error| BuildFailure {
        message: "Could not inspect Cargo build output".to_owned(),
        details: Some(error.to_string()),
        exit_code: Some(0),
    })? {
        let path = entry.path();
        let file_type = entry.file_type().await.map_err(|error| BuildFailure {
            message: "Could not inspect Cargo build output".to_owned(),
            details: Some(error.to_string()),
            exit_code: Some(0),
        })?;
        let extension = path.extension().and_then(|value| value.to_str());
        if file_type.is_file() && (extension.is_none() || extension == Some("elf")) {
            let length = entry.metadata().await.map(|meta| meta.len()).unwrap_or(0);
            candidates.push((length, path));
        }
    }
    candidates.sort_by_key(|(length, _)| *length);
    candidates
        .pop()
        .map(|(_, path)| path)
        .ok_or_else(|| BuildFailure {
            message: "Cargo succeeded but no ESP application ELF was found".to_owned(),
            details: Some(format!("Searched {}", release_dir.display())),
            exit_code: Some(0),
        })
}
