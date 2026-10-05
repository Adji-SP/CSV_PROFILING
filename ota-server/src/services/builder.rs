use std::{
    env,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
};

use async_trait::async_trait;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::{Mutex, mpsc::UnboundedSender},
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

#[derive(Clone)]
pub struct LocalFirmwareBuilder {
    build_dir: PathBuf,
    runtime_template_dir: PathBuf,
    cargo_target_dir: PathBuf,
    esp_idf_tools_dir: Option<PathBuf>,
    python_path: Option<PathBuf>,
    public_base_url: String,
    rust_toolchain: Option<String>,
    managed_environment: ManagedFirmwareEnvironment,
    build_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Default)]
struct ManagedFirmwareEnvironment {
    wifi_ssid: Option<String>,
    wifi_password: Option<String>,
    device_name: Option<String>,
    device_id: Option<String>,
}

impl ManagedFirmwareEnvironment {
    fn load() -> Self {
        Self {
            wifi_ssid: nonempty_env("WIFI_SSID"),
            wifi_password: nonempty_env("WIFI_PASS"),
            device_name: nonempty_env("DEVICE_NAME"),
            device_id: nonempty_env("DEVICE_ID"),
        }
    }

    fn apply(&self, command: &mut Command) -> Result<(), BuildFailure> {
        let ssid = self.wifi_ssid.as_deref().ok_or_else(|| {
            BuildFailure::new("WIFI_SSID is not configured for the managed firmware build")
        })?;
        let password = self.wifi_password.as_deref().ok_or_else(|| {
            BuildFailure::new("WIFI_PASS is not configured for the managed firmware build")
        })?;
        command.env("WIFI_SSID", ssid).env("WIFI_PASS", password);
        if let Some(name) = self.device_name.as_deref() {
            command.env("DEVICE_NAME", name);
        }
        if let Some(device_id) = self.device_id.as_deref() {
            command.env("DEVICE_ID", device_id);
        }
        Ok(())
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

impl LocalFirmwareBuilder {
    pub fn new(
        build_dir: PathBuf,
        runtime_template_dir: PathBuf,
        cargo_target_dir: PathBuf,
        esp_idf_tools_dir: Option<PathBuf>,
        python_path: Option<PathBuf>,
        public_base_url: String,
        rust_toolchain: Option<String>,
    ) -> Self {
        Self {
            build_dir,
            runtime_template_dir,
            cargo_target_dir,
            esp_idf_tools_dir,
            python_path,
            public_base_url,
            rust_toolchain,
            managed_environment: ManagedFirmwareEnvironment::load(),
            build_lock: Arc::new(Mutex::new(())),
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

    fn apply_esp_idf_environment(&self, command: &mut Command) -> Result<(), BuildFailure> {
        if let Some(directory) = self.esp_idf_tools_dir.as_ref() {
            command.env(
                "ESP_IDF_TOOLS_INSTALL_DIR",
                format!("custom:{}", directory.display()),
            );
        }
        if let Some(python) = self.python_path.as_ref() {
            let python_dir = python.parent().ok_or_else(|| {
                BuildFailure::new("OTA_PYTHON_PATH does not have a parent directory")
            })?;
            let mut paths = vec![python_dir.to_path_buf()];
            if let Some(existing) = env::var_os("PATH") {
                paths.extend(env::split_paths(&existing));
            }
            let joined = env::join_paths(paths).map_err(|error| BuildFailure {
                message: "Could not prepare PATH for the configured Python executable".to_owned(),
                details: Some(error.to_string()),
                exit_code: None,
            })?;
            command.env("PATH", joined);
        }
        Ok(())
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
        let managed_application = matches!(inspection.kind, ProjectKind::ManagedApplication);
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

        // esp-idf-sys rejects long Windows OUT_DIR paths. All local builds use a
        // deliberately short configured Cargo target directory. The lock keeps
        // composition-to-image generation isolated when builds are queued at once.
        let _build_guard = self.build_lock.lock().await;
        let target_dir = &self.cargo_target_dir;
        let _ = logs.send(format!("Cargo output directory: {}", target_dir.display()));
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
            // The human diagnostics remain on stderr. JSON artifact messages
            // identify the executable produced by this build, even when Cargo
            // reuses cached dependencies in a shared target directory.
            .arg("--message-format=json-render-diagnostics")
            .current_dir(&build_source)
            .env("CARGO_TARGET_DIR", target_dir)
            .env("FIRMWARE_VERSION", &request.version)
            .env("OTA_SERVER_URL", &self.public_base_url);
        if managed_application {
            self.managed_environment.apply(&mut cargo)?;
            let _ = logs.send(
                "Managed firmware network configuration loaded from the server environment"
                    .to_owned(),
            );
        }
        self.apply_esp_idf_environment(&mut cargo)?;
        if let Some(directory) = self.esp_idf_tools_dir.as_ref() {
            let _ = logs.send(format!(
                "ESP-IDF tools directory: {}",
                directory.display()
            ));
        }
        if self.python_path.is_some() {
            let _ = logs.send("Configured base Python added to the build PATH".to_owned());
        }
        self.apply_toolchain(&mut cargo);
        let (cargo_status, artifacts) =
            run_logged(cargo, logs.clone(), Some(build_source.join("Cargo.toml"))).await?;
        if !cargo_status.success() {
            return Err(BuildFailure::command_failed("Cargo build", cargo_status));
        }

        let release_dir = target_dir.join(spec.rust_target).join("release");
        let elf_path = select_elf(&release_dir, artifacts).await?;
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
        let (image_status, _) = run_logged(espflash, logs.clone(), None).await?;
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
    cargo_manifest: Option<PathBuf>,
) -> Result<(ExitStatus, Vec<PathBuf>), BuildFailure> {
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

    let stdout_task = tokio::spawn(forward_lines(stdout, logs.clone(), cargo_manifest));
    let stderr_task = tokio::spawn(forward_lines(stderr, logs, None));
    let status = child.wait().await.map_err(|error| BuildFailure {
        message: "Could not wait for build process".to_owned(),
        details: Some(error.to_string()),
        exit_code: None,
    })?;
    let artifacts = stdout_task.await.map_err(|error| BuildFailure {
        message: "Could not finish reading Cargo build output".to_owned(),
        details: Some(error.to_string()),
        exit_code: status.code(),
    })?;
    let _ = stderr_task.await;
    Ok((status, artifacts))
}

async fn forward_lines<R>(
    reader: R,
    logs: UnboundedSender<String>,
    cargo_manifest: Option<PathBuf>,
) -> Vec<PathBuf>
where
    R: AsyncRead + Unpin,
{
    let mut artifacts = Vec::new();
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(manifest) = cargo_manifest.as_deref() {
                    if let Some(message) = parse_cargo_line(&line, manifest) {
                        match message {
                            CargoLine::Artifact(path) => artifacts.push(path),
                            CargoLine::Diagnostic(rendered) => {
                                for line in rendered.lines() {
                                    let _ = logs.send(line.to_owned());
                                }
                            }
                            CargoLine::Other => {}
                        }
                        continue;
                    }
                }
                let _ = logs.send(line);
            }
            Ok(None) => break,
            Err(error) => {
                let _ = logs.send(format!("Log stream error: {error}"));
                break;
            }
        }
    }
    artifacts
}

enum CargoLine {
    Artifact(PathBuf),
    Diagnostic(String),
    Other,
}

fn parse_cargo_line(line: &str, expected_manifest: &Path) -> Option<CargoLine> {
    let value: Value = serde_json::from_str(line).ok()?;
    match value.get("reason")?.as_str()? {
        "compiler-artifact" => {
            let is_binary = value
                .pointer("/target/kind")
                .and_then(Value::as_array)
                .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")));
            if !is_binary {
                return Some(CargoLine::Other);
            }
            let Some(manifest) = value.get("manifest_path").and_then(Value::as_str) else {
                return Some(CargoLine::Other);
            };
            if !same_manifest(Path::new(manifest), expected_manifest) {
                return Some(CargoLine::Other);
            }
            Some(
                value
                    .get("executable")
                    .and_then(Value::as_str)
                    .map(|path| CargoLine::Artifact(PathBuf::from(path)))
                    .unwrap_or(CargoLine::Other),
            )
        }
        "compiler-message" => Some(
            value
                .pointer("/message/rendered")
                .and_then(Value::as_str)
                .map(|text| CargoLine::Diagnostic(text.to_owned()))
                .unwrap_or(CargoLine::Other),
        ),
        _ => Some(CargoLine::Other),
    }
}

fn same_manifest(candidate: &Path, expected: &Path) -> bool {
    match (candidate.canonicalize(), expected.canonicalize()) {
        (Ok(candidate), Ok(expected)) => candidate == expected,
        _ => false,
    }
}

async fn select_elf(release_dir: &Path, mut artifacts: Vec<PathBuf>) -> Result<PathBuf, BuildFailure> {
    artifacts.sort();
    artifacts.dedup();
    if artifacts.len() != 1 {
        return Err(BuildFailure {
            message: if artifacts.is_empty() {
                "Cargo succeeded but did not report an ESP application executable".to_owned()
            } else {
                "Cargo reported multiple application executables".to_owned()
            },
            details: Some(format!(
                "Expected one binary from the uploaded project in {}. If the project has multiple binaries, select a single firmware binary in Cargo.toml.",
                release_dir.display()
            )),
            exit_code: Some(0),
        });
    }
    let path = artifacts.pop().expect("one artifact was checked above");
    let canonical_release = tokio::fs::canonicalize(release_dir).await.map_err(|error| BuildFailure {
        message: "Cargo succeeded but the target release directory was not found".to_owned(),
        details: Some(error.to_string()),
        exit_code: Some(0),
    })?;
    let canonical_path = tokio::fs::canonicalize(&path).await.map_err(|error| BuildFailure {
        message: "Cargo reported an executable that does not exist".to_owned(),
        details: Some(error.to_string()),
        exit_code: Some(0),
    })?;
    let metadata = tokio::fs::metadata(&canonical_path).await.map_err(|error| BuildFailure {
        message: "Could not inspect the Cargo executable".to_owned(),
        details: Some(error.to_string()),
        exit_code: Some(0),
    })?;
    if !canonical_path.starts_with(&canonical_release) || !metadata.is_file() || metadata.len() == 0 {
        return Err(BuildFailure::new(
            "Cargo executable is empty or outside the configured target release directory",
        ));
    }
    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::{CargoLine, parse_cargo_line, select_elf};

    #[test]
    fn cargo_messages_only_select_current_projects_binary() {
        let temp = tempdir().expect("temporary directory");
        let current = temp.path().join("current/Cargo.toml");
        let stale = temp.path().join("stale/Cargo.toml");
        fs::create_dir_all(current.parent().expect("current directory")).expect("current dir");
        fs::create_dir_all(stale.parent().expect("stale directory")).expect("stale dir");
        fs::write(&current, "[package]\nname='current'\nversion='0.1.0'\n").expect("manifest");
        fs::write(&stale, "[package]\nname='stale'\nversion='0.1.0'\n").expect("manifest");

        let artifact = |manifest: &Path, kind: &str, executable: &str| {
            serde_json::json!({
                "reason": "compiler-artifact",
                "manifest_path": manifest,
                "target": {"kind": [kind]},
                "executable": executable,
                "fresh": true
            })
            .to_string()
        };
        assert!(matches!(
            parse_cargo_line(&artifact(&stale, "bin", "stale.elf"), &current),
            Some(CargoLine::Other)
        ));
        assert!(matches!(
            parse_cargo_line(&artifact(&current, "lib", "library.rlib"), &current),
            Some(CargoLine::Other)
        ));
        assert!(matches!(
            parse_cargo_line(&artifact(&current, "bin", "current.elf"), &current),
            Some(CargoLine::Artifact(path)) if path == Path::new("current.elf")
        ));
    }

    #[test]
    fn cargo_diagnostics_remain_readable() {
        let line = serde_json::json!({
            "reason": "compiler-message",
            "message": {"rendered": "error[E0425]: missing symbol\n --> src/main.rs:1:1\n"}
        })
        .to_string();
        assert!(matches!(
            parse_cargo_line(&line, Path::new("Cargo.toml")),
            Some(CargoLine::Diagnostic(text)) if text.contains("error[E0425]")
        ));
    }

    #[tokio::test]
    async fn never_selects_a_larger_stale_elf() {
        let temp = tempdir().expect("temporary directory");
        let release = temp.path().join("release");
        fs::create_dir_all(&release).expect("release directory");
        let current = release.join("current");
        let stale = release.join("stale");
        fs::write(&current, b"small current ELF").expect("current ELF");
        fs::write(&stale, vec![0_u8; 2048]).expect("stale ELF");

        let selected = select_elf(&release, vec![current.clone()])
            .await
            .expect("reported ELF should be selected");
        assert_eq!(selected, current.canonicalize().expect("canonical current ELF"));
        assert!(select_elf(&release, vec![]).await.is_err());
        assert!(select_elf(&release, vec![current, stale]).await.is_err());
    }
}
