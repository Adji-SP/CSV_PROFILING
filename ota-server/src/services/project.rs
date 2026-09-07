use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

pub const MANAGED_DESCRIPTOR: &str = "ota-app.toml";
pub const APPLICATION_API_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Standalone,
    ManagedApplication,
}

impl ProjectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::ManagedApplication => "managed_application",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectInspection {
    pub kind: ProjectKind,
    pub package_name: Option<String>,
    pub entrypoint: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ProjectValidationError {
    pub message: String,
    pub details: Option<String>,
}

impl ProjectValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: None,
        }
    }

    fn with_details(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: Some(details.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ManagedDescriptor {
    schema_version: u32,
    application_api_version: u32,
    #[serde(default = "default_entrypoint")]
    entrypoint: String,
}

fn default_entrypoint() -> String {
    "src/main.rs".to_owned()
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CargoManifest {
    package: CargoPackage,
    #[serde(default)]
    dependencies: toml::Table,
}

pub fn inspect_project(project_root: &Path) -> Result<ProjectInspection, ProjectValidationError> {
    validate_dependency_paths(project_root)?;

    let descriptor_path = project_root.join(MANAGED_DESCRIPTOR);
    if !descriptor_path.is_file() {
        return Ok(ProjectInspection {
            kind: ProjectKind::Standalone,
            package_name: None,
            entrypoint: None,
        });
    }

    let descriptor_text = fs::read_to_string(&descriptor_path).map_err(|error| {
        ProjectValidationError::with_details(
            format!("Could not read {MANAGED_DESCRIPTOR}"),
            error.to_string(),
        )
    })?;
    let descriptor: ManagedDescriptor = toml::from_str(&descriptor_text).map_err(|error| {
        ProjectValidationError::with_details(
            format!("{MANAGED_DESCRIPTOR} is invalid"),
            error.to_string(),
        )
    })?;
    if descriptor.schema_version != 1 {
        return Err(ProjectValidationError::new(format!(
            "Unsupported managed application schema version {}; expected 1",
            descriptor.schema_version
        )));
    }
    if descriptor.application_api_version != APPLICATION_API_VERSION {
        return Err(ProjectValidationError::new(format!(
            "Unsupported application API version {}; expected {APPLICATION_API_VERSION}",
            descriptor.application_api_version
        )));
    }

    let cargo_path = project_root.join("Cargo.toml");
    let cargo_text = fs::read_to_string(&cargo_path).map_err(|error| {
        ProjectValidationError::with_details(
            "Managed applications require a readable Cargo.toml at the project root",
            error.to_string(),
        )
    })?;
    let cargo: CargoManifest = toml::from_str(&cargo_text).map_err(|error| {
        ProjectValidationError::with_details(
            "Could not parse application Cargo.toml",
            error.to_string(),
        )
    })?;
    if !cargo.dependencies.contains_key("firmware-app-api") {
        return Err(ProjectValidationError::new(
            "Managed applications must declare firmware-app-api in [dependencies]",
        ));
    }

    let entrypoint = PathBuf::from(&descriptor.entrypoint);
    if entrypoint.extension().and_then(|value| value.to_str()) != Some("rs")
        || entrypoint
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProjectValidationError::new(
            "Managed application entrypoint must be a relative .rs path inside the project",
        ));
    }
    let entrypoint_path = project_root.join(&entrypoint);
    ensure_path_within(project_root, &entrypoint_path).map_err(|error| {
        ProjectValidationError::with_details(
            "Managed application entrypoint is invalid",
            error.message,
        )
    })?;
    if !entrypoint_path.is_file() {
        return Err(ProjectValidationError::new(format!(
            "Managed application entrypoint was not found: {}",
            entrypoint_path.display()
        )));
    }

    Ok(ProjectInspection {
        kind: ProjectKind::ManagedApplication,
        package_name: Some(cargo.package.name),
        entrypoint: Some(entrypoint),
    })
}

pub fn compose_managed_project(
    runtime_template: &Path,
    uploaded_project: &Path,
    destination: &Path,
    package_name: &str,
    entrypoint: &Path,
) -> Result<PathBuf, ProjectValidationError> {
    if !runtime_template.join(".ota-runtime-template").is_file() {
        return Err(ProjectValidationError::new(format!(
            "Managed runtime template marker is missing from {}",
            runtime_template.display()
        )));
    }
    if destination.exists() {
        ensure_path_within(uploaded_project, destination)?;
        fs::remove_dir_all(destination).map_err(|error| {
            ProjectValidationError::with_details(
                "Could not clear the previous managed build workspace",
                error.to_string(),
            )
        })?;
    }

    copy_tree(runtime_template, destination)?;
    let application_destination = destination.join("applications/device-app");
    fs::create_dir_all(&application_destination).map_err(|error| {
        ProjectValidationError::with_details(
            "Could not create managed application workspace",
            error.to_string(),
        )
    })?;
    copy_tree(uploaded_project, &application_destination)?;
    patch_runtime_manifest(destination, package_name)?;
    patch_application_manifest(&application_destination, entrypoint)?;
    Ok(destination.to_path_buf())
}

fn patch_runtime_manifest(
    destination: &Path,
    package_name: &str,
) -> Result<(), ProjectValidationError> {
    let path = destination.join("Cargo.toml");
    let mut manifest = read_toml(&path)?;
    let dependencies = manifest
        .get_mut("dependencies")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ProjectValidationError::new("Runtime template has no [dependencies] table")
        })?;
    let mut dependency = toml::Table::new();
    dependency.insert(
        "path".to_owned(),
        toml::Value::String("applications/device-app".to_owned()),
    );
    dependency.insert(
        "package".to_owned(),
        toml::Value::String(package_name.to_owned()),
    );
    dependencies.insert("device-app".to_owned(), toml::Value::Table(dependency));
    write_toml(&path, &manifest)
}

fn patch_application_manifest(
    destination: &Path,
    entrypoint: &Path,
) -> Result<(), ProjectValidationError> {
    let path = destination.join("Cargo.toml");
    let mut manifest = read_toml(&path)?;
    let dependencies = manifest
        .get_mut("dependencies")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ProjectValidationError::new("Managed application has no [dependencies] table")
        })?;
    let mut dependency = toml::Table::new();
    dependency.insert(
        "path".to_owned(),
        toml::Value::String("../../crates/application-api".to_owned()),
    );
    dependencies.insert(
        "firmware-app-api".to_owned(),
        toml::Value::Table(dependency),
    );
    let package = manifest
        .get_mut("package")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| ProjectValidationError::new("Managed application has no [package] table"))?;
    package.insert("autobins".to_owned(), toml::Value::Boolean(false));

    let mut library = manifest
        .get("lib")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    library.insert(
        "path".to_owned(),
        toml::Value::String(entrypoint.to_string_lossy().replace('\\', "/")),
    );
    library.insert(
        "crate-type".to_owned(),
        toml::Value::Array(vec![toml::Value::String("rlib".to_owned())]),
    );
    manifest
        .as_table_mut()
        .ok_or_else(|| ProjectValidationError::new("Cargo.toml root must be a table"))?
        .insert("lib".to_owned(), toml::Value::Table(library));
    write_toml(&path, &manifest)
}

fn read_toml(path: &Path) -> Result<toml::Value, ProjectValidationError> {
    let text = fs::read_to_string(path).map_err(|error| {
        ProjectValidationError::with_details(
            format!("Could not read {}", path.display()),
            error.to_string(),
        )
    })?;
    toml::from_str(&text).map_err(|error| {
        ProjectValidationError::with_details(
            format!("Could not parse {}", path.display()),
            error.to_string(),
        )
    })
}

fn write_toml(path: &Path, value: &toml::Value) -> Result<(), ProjectValidationError> {
    let text = toml::to_string_pretty(value).map_err(|error| {
        ProjectValidationError::with_details(
            "Could not serialize generated Cargo.toml",
            error.to_string(),
        )
    })?;
    fs::write(path, text).map_err(|error| {
        ProjectValidationError::with_details(
            format!("Could not write generated manifest {}", path.display()),
            error.to_string(),
        )
    })
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), ProjectValidationError> {
    fs::create_dir_all(destination).map_err(|error| {
        ProjectValidationError::with_details(
            format!("Could not create {}", destination.display()),
            error.to_string(),
        )
    })?;
    for entry in fs::read_dir(source).map_err(|error| {
        ProjectValidationError::with_details(
            format!("Could not inspect {}", source.display()),
            error.to_string(),
        )
    })? {
        let entry = entry.map_err(|error| {
            ProjectValidationError::with_details(
                "Could not inspect project entry",
                error.to_string(),
            )
        })?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some("target" | ".git" | ".ota-target" | ".ota-managed-runtime")
        ) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            ProjectValidationError::with_details(
                "Could not inspect project entry",
                error.to_string(),
            )
        })?;
        if file_type.is_symlink() {
            return Err(ProjectValidationError::new(
                "Symbolic links are not allowed in managed build workspaces",
            ));
        }
        let output = destination.join(&name);
        if file_type.is_dir() {
            copy_tree(&entry.path(), &output)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &output).map_err(|error| {
                ProjectValidationError::with_details(
                    format!("Could not copy {}", entry.path().display()),
                    error.to_string(),
                )
            })?;
        }
    }
    Ok(())
}

fn validate_dependency_paths(project_root: &Path) -> Result<(), ProjectValidationError> {
    let canonical_root = project_root.canonicalize().map_err(|error| {
        ProjectValidationError::with_details(
            "Could not validate project directory",
            error.to_string(),
        )
    })?;
    let mut directories = vec![project_root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| {
            ProjectValidationError::with_details(
                "Could not inspect project directory",
                error.to_string(),
            )
        })? {
            let entry = entry.map_err(|error| {
                ProjectValidationError::with_details(
                    "Could not inspect project entry",
                    error.to_string(),
                )
            })?;
            let file_type = entry.file_type().map_err(|error| {
                ProjectValidationError::with_details(
                    "Could not inspect project entry",
                    error.to_string(),
                )
            })?;
            if file_type.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | ".ota-target" | ".ota-managed-runtime")
                ) {
                    directories.push(entry.path());
                }
            } else if file_type.is_file() && entry.file_name() == "Cargo.toml" {
                validate_manifest_paths(&canonical_root, &entry.path())?;
            }
        }
    }
    Ok(())
}

fn validate_manifest_paths(
    canonical_root: &Path,
    manifest_path: &Path,
) -> Result<(), ProjectValidationError> {
    let manifest = read_toml(manifest_path)?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| ProjectValidationError::new("Cargo.toml has no containing directory"))?;
    let table = manifest
        .as_table()
        .ok_or_else(|| ProjectValidationError::new("Cargo.toml root must be a table"))?;
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(dependencies) = table.get(key).and_then(toml::Value::as_table) {
            validate_dependency_table(canonical_root, manifest_dir, dependencies)?;
        }
    }
    if let Some(workspace_dependencies) = table
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
    {
        validate_dependency_table(canonical_root, manifest_dir, workspace_dependencies)?;
    }
    if let Some(targets) = table.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(dependencies) = target.get(key).and_then(toml::Value::as_table) {
                    validate_dependency_table(canonical_root, manifest_dir, dependencies)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_dependency_table(
    canonical_root: &Path,
    manifest_dir: &Path,
    dependencies: &toml::Table,
) -> Result<(), ProjectValidationError> {
    for (name, specification) in dependencies {
        let Some(path_value) = specification
            .as_table()
            .and_then(|table| table.get("path"))
            .and_then(toml::Value::as_str)
        else {
            continue;
        };
        let relative = Path::new(path_value);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
        {
            return Err(ProjectValidationError::new(format!(
                "Dependency '{name}' uses an absolute path, which is not allowed"
            )));
        }
        let candidate = manifest_dir
            .join(relative)
            .canonicalize()
            .map_err(|error| {
                ProjectValidationError::with_details(
                    format!("Dependency '{name}' points to a missing local path"),
                    error.to_string(),
                )
            })?;
        if !candidate.starts_with(canonical_root) {
            return Err(ProjectValidationError::new(format!(
                "Dependency '{name}' points outside the uploaded project"
            )));
        }
    }
    Ok(())
}

fn ensure_path_within(root: &Path, candidate: &Path) -> Result<(), ProjectValidationError> {
    let canonical_root = root.canonicalize().map_err(|error| {
        ProjectValidationError::with_details("Could not validate project root", error.to_string())
    })?;
    let resolved = if candidate.exists() {
        candidate.canonicalize().map_err(|error| {
            ProjectValidationError::with_details(
                "Could not validate project path",
                error.to_string(),
            )
        })?
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| ProjectValidationError::new("Generated project path has no parent"))?;
        let canonical_parent = parent.canonicalize().map_err(|error| {
            ProjectValidationError::with_details(
                "Could not validate generated project path",
                error.to_string(),
            )
        })?;
        canonical_parent.join(candidate.file_name().unwrap_or_default())
    };
    if !resolved.starts_with(canonical_root) {
        return Err(ProjectValidationError::new(
            "Generated project path is outside the uploaded project",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{ProjectKind, compose_managed_project, inspect_project};

    fn write_managed_app(root: &std::path::Path, name: &str) {
        fs::create_dir_all(root.join("src")).expect("create source directory");
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfirmware-app-api = \"0.1\"\n"
            ),
        )
        .expect("write Cargo manifest");
        fs::write(
            root.join("src/main.rs"),
            "pub fn setup() -> Result<(), ()> { Ok(()) }\npub fn main(_: ()) {}\n",
        )
        .expect("write functional entrypoint");
        fs::write(
            root.join("ota-app.toml"),
            "schema_version = 1\napplication_api_version = 2\nentrypoint = 'src/main.rs'\n",
        )
        .expect("write descriptor");
    }

    #[test]
    fn detects_managed_application() {
        let directory = tempdir().expect("temporary directory");
        write_managed_app(directory.path(), "sample-app");
        let inspection = inspect_project(directory.path()).expect("inspect project");
        assert_eq!(inspection.kind, ProjectKind::ManagedApplication);
        assert_eq!(inspection.package_name.as_deref(), Some("sample-app"));
        assert_eq!(
            inspection.entrypoint.as_deref(),
            Some(std::path::Path::new("src/main.rs"))
        );
    }

    #[test]
    fn rejects_obsolete_application_api() {
        let directory = tempdir().expect("temporary directory");
        write_managed_app(directory.path(), "old-app");
        fs::write(
            directory.path().join("ota-app.toml"),
            "schema_version = 1\napplication_api_version = 1\nentrypoint = 'src/main.rs'\n",
        )
        .expect("write old descriptor");

        let error = inspect_project(directory.path()).expect_err("old API must fail");
        assert!(error.message.contains("expected 2"));
    }

    #[test]
    fn rejects_entrypoint_path_traversal() {
        let directory = tempdir().expect("temporary directory");
        write_managed_app(directory.path(), "unsafe-entrypoint");
        fs::write(
            directory.path().join("ota-app.toml"),
            "schema_version = 1\napplication_api_version = 2\nentrypoint = '../main.rs'\n",
        )
        .expect("write unsafe descriptor");

        let error = inspect_project(directory.path()).expect_err("traversal must fail");
        assert!(error.message.contains("relative .rs path"));
    }

    #[test]
    fn accepts_legacy_standalone_project() {
        let directory = tempdir().expect("temporary directory");
        fs::create_dir_all(directory.path().join("src")).expect("create source directory");
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname='standalone'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("write Cargo manifest");
        fs::write(directory.path().join("src/main.rs"), "fn main() {}\n")
            .expect("write main source");

        let inspection = inspect_project(directory.path()).expect("inspect project");
        assert_eq!(inspection.kind, ProjectKind::Standalone);
        assert_eq!(inspection.package_name, None);
        assert_eq!(inspection.entrypoint, None);
    }

    #[test]
    fn rejects_external_path_dependency() {
        let parent = tempdir().expect("temporary directory");
        let app = parent.path().join("app");
        write_managed_app(&app, "unsafe-app");
        fs::create_dir_all(parent.path().join("outside/src")).expect("create outside crate");
        fs::write(
            parent.path().join("outside/Cargo.toml"),
            "[package]\nname='outside'\nversion='0.1.0'\n",
        )
        .expect("write outside manifest");
        fs::write(
            app.join("Cargo.toml"),
            "[package]\nname='unsafe-app'\nversion='0.1.0'\n\n[dependencies]\nfirmware-app-api='0.1'\noutside={path='../outside'}\n",
        )
        .expect("write unsafe manifest");
        let error = inspect_project(&app).expect_err("external path must fail");
        assert!(error.message.contains("outside the uploaded project"));
    }

    #[test]
    fn composes_runtime_without_editing_upload() {
        let directory = tempdir().expect("temporary directory");
        let runtime = directory.path().join("runtime");
        let app = directory.path().join("app");
        fs::create_dir_all(runtime.join("src")).expect("runtime source");
        fs::write(runtime.join(".ota-runtime-template"), "managed runtime\n")
            .expect("runtime marker");
        fs::write(
            runtime.join("Cargo.toml"),
            "[package]\nname='runtime'\nversion='0.1.0'\n\n[dependencies]\ndevice-app={path='applications/default-app'}\n",
        )
        .expect("runtime manifest");
        fs::write(runtime.join("src/main.rs"), "fn main() {}\n").expect("runtime main");
        write_managed_app(&app, "sample-app");
        let original = fs::read_to_string(app.join("Cargo.toml")).expect("original manifest");
        let destination = app.join(".ota-managed-runtime");
        compose_managed_project(
            &runtime,
            &app,
            &destination,
            "sample-app",
            std::path::Path::new("src/main.rs"),
        )
        .expect("compose runtime");
        let generated =
            fs::read_to_string(destination.join("Cargo.toml")).expect("generated manifest");
        assert!(generated.contains("package = \"sample-app\""));
        let generated_app =
            fs::read_to_string(destination.join("applications/device-app/Cargo.toml"))
                .expect("generated app manifest");
        assert!(generated_app.contains("../../crates/application-api"));
        assert!(generated_app.contains("autobins = false"));
        assert!(generated_app.contains("path = \"src/main.rs\""));
        assert!(generated_app.contains("crate-type = [\"rlib\"]"));
        assert_eq!(
            fs::read_to_string(app.join("Cargo.toml")).expect("unchanged upload"),
            original
        );
    }
}
