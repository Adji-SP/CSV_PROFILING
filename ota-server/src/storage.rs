use std::time::Duration;

use chrono::Utc;
use sqlx::{AnyPool, any::AnyPoolOptions};

use crate::{
    errors::{ApiError, ApiResult},
    models::{Build, Device, Firmware, Project},
};

#[derive(Debug, Clone)]
pub struct Storage {
    pool: AnyPool,
}

const UNCONFIRMED_BOOT_ERROR: &str =
    "Device returned with a different version after OTA reboot; possible rollback or USB recovery";

impl Storage {
    pub async fn connect(database_url: &str) -> ApiResult<Self> {
        let database_url = if database_url.starts_with("sqlite:") {
            // Any parses a URL first: a Windows drive must not become a URL host.
            database_url
                .replace("\\\\?\\", "")
                .replace('\\', "/")
                .replacen("sqlite://", "sqlite:", 1)
        } else {
            database_url.to_owned()
        };
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    if conn.backend_name() == "SQLite" {
                        sqlx::query("PRAGMA foreign_keys=ON")
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query("PRAGMA busy_timeout=5000")
                            .execute(&mut *conn)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(&database_url)
            .await?;
        let storage = Self { pool };
        // Reports hold a read snapshot while telemetry continues writing.
        if database_url.starts_with("sqlite:") {
            sqlx::query("PRAGMA journal_mode=WAL")
                .execute(&storage.pool)
                .await?;
        }
        storage.migrate().await?;
        Ok(storage)
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    async fn migrate(&self) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_versions (version BIGINT PRIMARY KEY)")
            .execute(&mut *tx)
            .await?;
        let applied: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM schema_versions WHERE version=1")
                .fetch_one(&mut *tx)
                .await?;
        if applied == 0 {
            for statement in include_str!("../migrations/001_initial.sql")
                .split(';')
                .filter(|s| !s.trim().is_empty())
            {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO schema_versions(version) VALUES (1)")
                .execute(&mut *tx)
                .await?;
        }
        let tinyml_applied: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM schema_versions WHERE version=2")
                .fetch_one(&mut *tx)
                .await?;
        if tinyml_applied == 0 {
            for statement in include_str!("../migrations/002_tinyml.sql")
                .split(';')
                .filter(|s| !s.trim().is_empty())
            {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO schema_versions(version) VALUES (2)")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn insert_project(&self, project: &Project) -> ApiResult<()> {
        sqlx::query(
            "INSERT INTO projects (id,name,target,version,path,status,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(&project.id)
        .bind(&project.name)
        .bind(&project.target)
        .bind(&project.version)
        .bind(&project.path)
        .bind(&project.status)
        .bind(&project.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_project(&self, id: &str) -> ApiResult<Project> {
        sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Firmware project not found"))
    }

    pub async fn list_projects(&self) -> ApiResult<Vec<Project>> {
        Ok(
            sqlx::query_as::<_, Project>("SELECT * FROM projects ORDER BY created_at DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn set_project_status(&self, id: &str, status: &str) -> ApiResult<()> {
        sqlx::query("UPDATE projects SET status = $1 WHERE id = $2")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_build(&self, id: &str, project_id: &str, status: &str) -> ApiResult<()> {
        sqlx::query("INSERT INTO builds (id,project_id,status,created_at) VALUES ($1,$2,$3,$4)")
            .bind(id)
            .bind(project_id)
            .bind(status)
            .bind(now())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Reserve one build for a project in a single transaction. Updating the
    /// project row first serializes competing requests on both SQLite and
    /// PostgreSQL; checking for an active build also covers older records that
    /// were created before the project status was kept in sync.
    pub async fn reserve_build(&self, project_id: &str, build_id: &str) -> ApiResult<Project> {
        let mut tx = self.pool.begin().await?;
        let claimed = sqlx::query(
            r#"UPDATE projects SET status='queued' WHERE id=$1
            AND status NOT IN ('queued','building')
            AND NOT EXISTS (SELECT 1 FROM builds WHERE project_id=$1
                AND status IN ('queued','building'))"#,
        )
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() == 0 {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id=$1")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
            return Err(if exists == 0 {
                ApiError::not_found("Firmware project not found")
            } else {
                ApiError::conflict(
                    "BUILD_ALREADY_RUNNING",
                    "This project already has a queued or active build",
                )
            });
        }
        sqlx::query(
            "INSERT INTO builds (id,project_id,status,logs_text,created_at) VALUES ($1,$2,'queued',$3,$4)",
        )
        .bind(build_id)
        .bind(project_id)
        .bind("Build queued\n")
        .bind(now())
        .execute(&mut *tx)
        .await?;
        let project = sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE id=$1")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(project)
    }

    /// Called once by the full OTA server at startup. Build processes cannot
    /// survive as managed jobs across server restarts, so previous in-flight
    /// records must become retryable failures instead of blocking new builds.
    pub async fn reconcile_interrupted_builds(&self) -> ApiResult<u64> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"UPDATE projects SET status='failed' WHERE id IN
            (SELECT project_id FROM builds WHERE status IN ('queued','building'))"#,
        )
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"UPDATE builds SET status='failed',finished_at=$1,
            error='Build interrupted by OTA server restart',
            logs_text=logs_text || $2 WHERE status IN ('queued','building')"#,
        )
        .bind(now())
        .bind("BUILD FAILED: Build interrupted by OTA server restart. Retry the build.\n")
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn get_build(&self, id: &str) -> ApiResult<Build> {
        let query = r#"SELECT b.id,b.project_id,p.name AS project_name,p.target,p.version,
            b.status,b.logs_text,b.exit_code,b.error,b.started_at,b.finished_at,b.created_at
            FROM builds b JOIN projects p ON p.id=b.project_id WHERE b.id=$1"#;
        sqlx::query_as::<_, Build>(query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Build not found"))
    }

    pub async fn list_builds(&self) -> ApiResult<Vec<Build>> {
        let query = r#"SELECT b.id,b.project_id,p.name AS project_name,p.target,p.version,
            b.status,b.logs_text,b.exit_code,b.error,b.started_at,b.finished_at,b.created_at
            FROM builds b JOIN projects p ON p.id=b.project_id ORDER BY b.created_at DESC"#;
        Ok(sqlx::query_as::<_, Build>(query)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn start_build(&self, id: &str) -> ApiResult<()> {
        sqlx::query("UPDATE builds SET status='building',started_at=$1 WHERE id=$2")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn append_build_log(&self, id: &str, line: &str) -> ApiResult<()> {
        let safe_line = line.replace('\0', "");
        sqlx::query("UPDATE builds SET logs_text = logs_text || $1 WHERE id=$2")
            .bind(format!("{safe_line}\n"))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn finish_build(
        &self,
        id: &str,
        status: &str,
        exit_code: Option<i32>,
        error: Option<&str>,
    ) -> ApiResult<()> {
        sqlx::query("UPDATE builds SET status=$1,exit_code=$2,error=$3,finished_at=$4 WHERE id=$5")
            .bind(status)
            .bind(exit_code)
            .bind(error)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_firmware(&self, firmware: &Firmware) -> ApiResult<()> {
        sqlx::query(r#"INSERT INTO firmware
            (id,project_id,build_id,project,target,version,filename,path,file_size,sha256,status,created_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)"#)
            .bind(&firmware.id)
            .bind(&firmware.project_id)
            .bind(&firmware.build_id)
            .bind(&firmware.project)
            .bind(&firmware.target)
            .bind(&firmware.version)
            .bind(&firmware.filename)
            .bind(&firmware.path)
            .bind(firmware.file_size)
            .bind(&firmware.sha256)
            .bind(&firmware.status)
            .bind(&firmware.created_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_firmware(&self, id: &str) -> ApiResult<Firmware> {
        sqlx::query_as::<_, Firmware>("SELECT * FROM firmware WHERE id=$1 AND status='ready'")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Firmware not found"))
    }

    pub async fn list_firmware(&self) -> ApiResult<Vec<Firmware>> {
        Ok(
            sqlx::query_as::<_, Firmware>("SELECT * FROM firmware WHERE status<>'deleted' ORDER BY created_at DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn latest_firmware(&self, target: &str) -> ApiResult<Firmware> {
        sqlx::query_as::<_, Firmware>(
            "SELECT * FROM firmware WHERE target=$1 AND status='ready' ORDER BY created_at DESC LIMIT 1",
        )
        .bind(target)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("No ready firmware exists for this target"))
    }

    /// Reserve deletion before touching the file. A tombstone keeps deployment
    /// history intact and makes interrupted filesystem cleanup safe to retry.
    pub async fn begin_firmware_deletion(&self, id: &str) -> ApiResult<Firmware> {
        let mut tx = self.pool.begin().await?;
        let reserved = sqlx::query(
            "UPDATE firmware SET status='deleting' WHERE id=$1 AND status IN ('ready','deleting')",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if reserved.rows_affected() == 0 {
            return Err(ApiError::not_found("Firmware not found"));
        }
        let firmware = sqlx::query_as::<_, Firmware>("SELECT * FROM firmware WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        // Until devices report image hashes, matching chip + version is the
        // conservative way to protect their currently running recovery image.
        let devices: Vec<String> = sqlx::query_scalar(
            "SELECT device_id FROM devices WHERE desired_firmware_id=$1 OR (chip=$2 AND current_version=$3)",
        )
        .bind(id)
        .bind(&firmware.target)
        .bind(&firmware.version)
        .fetch_all(&mut *tx)
        .await?;
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM deployments WHERE firmware_id=$1 AND status IN ('pending','downloading','verifying','installing','rebooting')",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !devices.is_empty() || active > 0 {
            return Err(ApiError::conflict(
                "FIRMWARE_IN_USE",
                format!(
                    "Firmware is queued or currently running on a device ({}). Cancel the queued update or deploy another version first. Past deployments do not block deletion.",
                    if devices.is_empty() { "active deployment".to_owned() } else { devices.join(", ") }
                ),
            ));
        }
        tx.commit().await?;
        Ok(firmware)
    }

    pub async fn finish_firmware_deletion(&self, id: &str) -> ApiResult<()> {
        sqlx::query("UPDATE firmware SET status='deleted' WHERE id=$1 AND status='deleting'")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn upsert_device(
        &self,
        device_id: &str,
        name: &str,
        chip: &str,
        current_version: &str,
        ip_address: &str,
    ) -> ApiResult<Device> {
        sqlx::query(r#"INSERT INTO devices
            (device_id,name,chip,ip_address,current_version,status,ota_status,ota_progress,last_seen)
            VALUES ($1,$2,$3,$4,$5,'online','idle',0,$6)
            ON CONFLICT(device_id) DO UPDATE SET
              name=excluded.name, chip=excluded.chip, ip_address=excluded.ip_address,
              current_version=excluded.current_version, status='online', last_seen=excluded.last_seen"#)
            .bind(device_id)
            .bind(name)
            .bind(chip)
            .bind(ip_address)
            .bind(current_version)
            .bind(now())
            .execute(&self.pool)
            .await?;
        self.reconcile_booted_deployment(device_id, current_version)
            .await?;
        self.get_device(device_id).await
    }

    pub async fn get_device(&self, device_id: &str) -> ApiResult<Device> {
        sqlx::query_as::<_, Device>("SELECT * FROM devices WHERE device_id=$1")
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Device not found"))
    }

    pub async fn list_devices(&self) -> ApiResult<Vec<Device>> {
        Ok(
            sqlx::query_as::<_, Device>("SELECT * FROM devices ORDER BY last_seen DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn heartbeat_device(
        &self,
        device_id: &str,
        current_version: &str,
        status: &str,
        ip_address: &str,
    ) -> ApiResult<Device> {
        let result = sqlx::query(
            r#"UPDATE devices SET
            current_version=$1,status=$2,ip_address=$3,last_seen=$4 WHERE device_id=$5"#,
        )
        .bind(current_version)
        .bind(status)
        .bind(ip_address)
        .bind(now())
        .bind(device_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("Device not found; register it first"));
        }
        self.reconcile_booted_deployment(device_id, current_version)
            .await?;
        self.get_device(device_id).await
    }

    /// A successful boot can be reported by registration/heartbeat before the
    /// separate OTA-success callback is sent. Reconcile only the queued version.
    async fn reconcile_booted_deployment(
        &self,
        device_id: &str,
        current_version: &str,
    ) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        let queued: Option<(Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT desired_version,desired_firmware_id,ota_status FROM devices WHERE device_id=$1",
        )
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((Some(version), Some(firmware_id), ota_status)) = queued {
            if version == current_version {
                let changed = sqlx::query(
                    r#"UPDATE devices SET desired_version=NULL,desired_firmware_id=NULL,
                    ota_status='success',ota_progress=100,ota_error=NULL
                    WHERE device_id=$1 AND desired_version=$2 AND desired_firmware_id=$3"#,
                )
                .bind(device_id)
                .bind(&version)
                .bind(&firmware_id)
                .execute(&mut *tx)
                .await?;
                if changed.rows_affected() == 1 {
                    sqlx::query(
                        r#"UPDATE deployments SET status='success',progress=100,error=NULL,updated_at=$1
                        WHERE id=(SELECT id FROM deployments WHERE device_id=$2 AND firmware_id=$3
                        ORDER BY created_at DESC LIMIT 1)"#,
                    )
                    .bind(now())
                    .bind(device_id)
                    .bind(&firmware_id)
                    .execute(&mut *tx)
                    .await?;
                }
            } else if ota_status == "rebooting" {
                // The requested image never reported a healthy boot. Avoid
                // reinstalling it forever when the previous slot returns.
                let changed = sqlx::query(
                    r#"UPDATE devices SET desired_version=NULL,desired_firmware_id=NULL,
                    ota_status='failed',ota_progress=0,ota_error=$1
                    WHERE device_id=$2 AND desired_version=$3 AND desired_firmware_id=$4"#,
                )
                .bind(UNCONFIRMED_BOOT_ERROR)
                .bind(device_id)
                .bind(&version)
                .bind(&firmware_id)
                .execute(&mut *tx)
                .await?;
                if changed.rows_affected() == 1 {
                    sqlx::query(
                        r#"UPDATE deployments SET status='failed',progress=0,error=$1,updated_at=$2
                        WHERE id=(SELECT id FROM deployments WHERE device_id=$3 AND firmware_id=$4
                        ORDER BY created_at DESC LIMIT 1)"#,
                    )
                    .bind(UNCONFIRMED_BOOT_ERROR)
                    .bind(now())
                    .bind(device_id)
                    .bind(&firmware_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn create_deployment(
        &self,
        deployment_id: &str,
        device_id: &str,
        firmware: &Firmware,
    ) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        // Serialize against deletion, including a deploy handler that fetched
        // the ready image just before another request reserved its deletion.
        let available = sqlx::query("UPDATE firmware SET status=status WHERE id=$1 AND status='ready'")
            .bind(&firmware.id)
            .execute(&mut *tx)
            .await?;
        if available.rows_affected() == 0 {
            return Err(ApiError::conflict("FIRMWARE_NOT_READY", "Firmware is being deleted or is no longer available"));
        }
        sqlx::query(
            r#"UPDATE deployments SET status='superseded',updated_at=$1
            WHERE device_id=$2 AND status IN ('pending','downloading','verifying','installing','rebooting')"#,
        )
        .bind(now())
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO deployments
            (id,device_id,firmware_id,status,progress,created_at,updated_at)
            VALUES ($1,$2,$3,'pending',0,$4,$5)"#,
        )
        .bind(deployment_id)
        .bind(device_id)
        .bind(&firmware.id)
        .bind(now())
        .bind(now())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE devices SET desired_version=$1,desired_firmware_id=$2,
            ota_status='pending',ota_progress=0,ota_error=NULL WHERE device_id=$3"#,
        )
        .bind(&firmware.version)
        .bind(&firmware.id)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn cancel_update(&self, device_id: &str) -> ApiResult<Device> {
        let mut tx = self.pool.begin().await?;
        let desired_firmware_id: Option<Option<String>> = sqlx::query_scalar(
            "SELECT desired_firmware_id FROM devices WHERE device_id=$1",
        )
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(desired_firmware_id) = desired_firmware_id else {
            return Err(ApiError::not_found("Device not found"));
        };
        if let Some(firmware_id) = desired_firmware_id {
            sqlx::query(
                r#"UPDATE deployments SET status='cancelled',updated_at=$1
                WHERE id=(SELECT id FROM deployments WHERE device_id=$2 AND firmware_id=$3
                ORDER BY created_at DESC LIMIT 1)"#,
            )
            .bind(now())
            .bind(device_id)
            .bind(&firmware_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"UPDATE devices SET desired_version=NULL,desired_firmware_id=NULL,
                ota_status='idle',ota_progress=0,ota_error=NULL WHERE device_id=$1"#,
            )
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.get_device(device_id).await
    }

    pub async fn update_ota_status(
        &self,
        device_id: &str,
        status: &str,
        progress: i64,
        current_version: Option<&str>,
        error: Option<&str>,
    ) -> ApiResult<Device> {
        let mut tx = self.pool.begin().await?;
        let desired: Option<(Option<String>, Option<String>, String, String)> = sqlx::query_as(
            "SELECT desired_version,desired_firmware_id,current_version,ota_status FROM devices WHERE device_id=$1",
        )
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((desired_version, desired_firmware_id, stored_version, stored_ota_status)) = desired else {
            return Err(ApiError::not_found("Device not found"));
        };
        if status == "success" {
            if let Some(version) = desired_version.as_deref() {
                if current_version != Some(version) {
                    return Err(ApiError::conflict(
                        "OTA_VERSION_MISMATCH",
                        "The reported booted version does not match the queued firmware",
                    ));
                }
            } else if current_version != Some(stored_version.as_str())
                || stored_ota_status == "failed"
            {
                // A success callback may follow registration, which already
                // reconciled the deployment. It may not invent a new version.
                return Err(ApiError::conflict(
                    "NO_MATCHING_DEPLOYMENT",
                    "No queued deployment or registered boot matches this version",
                ));
            }
        }
        if status == "success" {
            sqlx::query(
                r#"UPDATE devices SET ota_status='success',ota_progress=100,
                ota_error=NULL,current_version=COALESCE($1,current_version),
                desired_version=NULL,desired_firmware_id=NULL,last_seen=$2 WHERE device_id=$3"#,
            )
            .bind(current_version)
            .bind(now())
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                r#"UPDATE devices SET ota_status=$1,ota_progress=$2,ota_error=$3,
                current_version=COALESCE($4,current_version),last_seen=$5 WHERE device_id=$6"#,
            )
            .bind(status)
            .bind(progress)
            .bind(error)
            .bind(current_version)
            .bind(now())
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(firmware_id) = desired_firmware_id {
            sqlx::query(r#"UPDATE deployments SET status=$1,progress=$2,error=$3,updated_at=$4
                WHERE id=(SELECT id FROM deployments WHERE device_id=$5 AND firmware_id=$6
                ORDER BY created_at DESC LIMIT 1)"#)
                .bind(status)
                .bind(progress)
                .bind(error)
                .bind(now())
                .bind(device_id)
                .bind(firmware_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.get_device(device_id).await
    }
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn exercise(url: &str) {
        let storage = Storage::connect(url)
            .await
            .expect("database connection and migration");
        storage.migrate().await.expect("idempotent migration");
        let id = uuid::Uuid::new_v4().to_string();
        storage
            .upsert_device(&id, "Console test", "esp32s3", "1.0", "127.0.0.1")
            .await
            .expect("register");
        let device = storage.get_device(&id).await.expect("read");
        assert_eq!(device.current_version, "1.0");
        assert_eq!(device.ota_progress, 0);
        storage
            .heartbeat_device(&id, "1.1", "online", "127.0.0.2")
            .await
            .expect("heartbeat");
        assert_eq!(
            storage.get_device(&id).await.expect("read").current_version,
            "1.1"
        );
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: "test".into(),
            target: "esp32s3".into(),
            version: "1.2".into(),
            path: "test-project".into(),
            status: "uploaded".into(),
            created_at: now(),
        };
        storage.insert_project(&project).await.expect("project");
        let build_id = uuid::Uuid::new_v4().to_string();
        storage
            .insert_build(&build_id, &project.id, "queued")
            .await
            .expect("build");
        storage.start_build(&build_id).await.expect("start");
        storage
            .append_build_log(&build_id, "compiler line")
            .await
            .expect("log");
        storage
            .finish_build(&build_id, "success", Some(0), None)
            .await
            .expect("finish");
        let build = storage.get_build(&build_id).await.expect("read build");
        assert_eq!(build.exit_code, Some(0));
        assert_eq!(build.logs_text, "compiler line\n");
        let firmware = Firmware {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project.id.clone(),
            build_id: build_id.clone(),
            project: "test".into(),
            target: "esp32s3".into(),
            version: "1.2".into(),
            filename: format!("{build_id}.bin"),
            path: "test.bin".into(),
            file_size: 4096,
            sha256: "a".repeat(64),
            status: "ready".into(),
            created_at: now(),
        };
        storage.insert_firmware(&firmware).await.expect("firmware");
        assert_eq!(
            storage
                .get_firmware(&firmware.id)
                .await
                .expect("read firmware")
                .file_size,
            4096
        );
        storage
            .create_deployment(&uuid::Uuid::new_v4().to_string(), &id, &firmware)
            .await
            .expect("deploy");
        assert_eq!(storage.begin_firmware_deletion(&firmware.id).await.expect_err("queued image is protected").code, "FIRMWARE_IN_USE");
        storage
            .update_ota_status(&id, "downloading", 34, None, None)
            .await
            .expect("progress");
        assert_eq!(
            storage.get_device(&id).await.expect("device").ota_progress,
            34
        );
        storage
            .update_ota_status(&id, "success", 100, Some("1.2"), None)
            .await
            .expect("success");
        assert_eq!(
            storage
                .get_device(&id)
                .await
                .expect("device")
                .current_version,
            "1.2"
        );
        sqlx::query("DELETE FROM devices WHERE device_id=$1")
            .bind(id)
            .execute(storage.pool())
            .await
            .expect("cleanup");
        storage.begin_firmware_deletion(&firmware.id).await.expect("reserve deletion");
        storage.finish_firmware_deletion(&firmware.id).await.expect("delete firmware");
        // Physical removal is only for cleaning this disposable test database.
        sqlx::query("DELETE FROM firmware WHERE id=$1").bind(&firmware.id).execute(storage.pool()).await.expect("cleanup firmware");
        sqlx::query("DELETE FROM builds WHERE id=$1")
            .bind(build_id)
            .execute(storage.pool())
            .await
            .expect("cleanup build");
        sqlx::query("DELETE FROM projects WHERE id=$1")
            .bind(project.id)
            .execute(storage.pool())
            .await
            .expect("cleanup project");
    }
    #[tokio::test]
    async fn sqlite_regression() {
        let dir = tempfile::tempdir().expect("temporary directory");
        exercise(&format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("test.db").display()
        ))
        .await;
    }

    #[tokio::test]
    async fn sqlite_build_reservation_is_atomic_per_project() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let storage = Storage::connect(&format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("builds.db").display()
        ))
        .await
        .expect("temporary database");
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: "concurrent".into(),
            target: "esp32s3".into(),
            version: "3.0.3".into(),
            path: "test-project".into(),
            status: "uploaded".into(),
            created_at: now(),
        };
        storage.insert_project(&project).await.expect("project");

        let mut requests = Vec::new();
        for _ in 0..12 {
            let storage = storage.clone();
            let project_id = project.id.clone();
            requests.push(tokio::spawn(async move {
                storage
                    .reserve_build(&project_id, &uuid::Uuid::new_v4().to_string())
                    .await
            }));
        }
        let mut accepted = 0;
        for request in requests {
            match request.await.expect("request task") {
                Ok(_) => accepted += 1,
                Err(error) => assert_eq!(error.code, "BUILD_ALREADY_RUNNING"),
            }
        }
        assert_eq!(accepted, 1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM builds WHERE project_id=$1")
            .bind(&project.id)
            .fetch_one(storage.pool())
            .await
            .expect("build count");
        assert_eq!(count, 1);
        assert_eq!(storage.get_project(&project.id).await.expect("project").status, "queued");
    }

    #[tokio::test]
    async fn sqlite_interrupted_builds_fail_once_and_can_be_retried() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let storage = Storage::connect(&format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("restart.db").display()
        ))
        .await
        .expect("temporary database");
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: "interrupted".into(),
            target: "esp32s3".into(),
            version: "3.0.3".into(),
            path: "test-project".into(),
            status: "uploaded".into(),
            created_at: now(),
        };
        storage.insert_project(&project).await.expect("project");
        let first = uuid::Uuid::new_v4().to_string();
        storage.reserve_build(&project.id, &first).await.expect("reserve");
        storage.start_build(&first).await.expect("start");
        storage.set_project_status(&project.id, "building").await.expect("project status");

        assert_eq!(storage.reconcile_interrupted_builds().await.expect("reconcile"), 1);
        let interrupted = storage.get_build(&first).await.expect("build");
        assert_eq!(interrupted.status, "failed");
        assert!(interrupted.logs_text.contains("interrupted by OTA server restart"));
        assert!(interrupted.finished_at.is_some());
        assert_eq!(storage.get_project(&project.id).await.expect("project").status, "failed");
        assert_eq!(storage.reconcile_interrupted_builds().await.expect("idempotent"), 0);

        let retry = uuid::Uuid::new_v4().to_string();
        storage.reserve_build(&project.id, &retry).await.expect("retry");
        assert_eq!(storage.get_build(&retry).await.expect("retry build").status, "queued");
    }

    #[tokio::test]
    async fn sqlite_cancel_and_reconcile_deployments() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let storage = Storage::connect(&format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("deployments.db").display()
        ))
        .await
        .expect("database");
        let device_id = "esp32-test";
        storage
            .upsert_device(device_id, "Test", "esp32s3", "1.0.0", "127.0.0.1")
            .await
            .expect("register");
        let project_id = uuid::Uuid::new_v4().to_string();
        storage
            .insert_project(&Project {
                id: project_id.clone(),
                name: "test".into(),
                target: "esp32s3".into(),
                version: "1.1.0".into(),
                path: "test".into(),
                status: "uploaded".into(),
                created_at: now(),
            })
            .await
            .expect("project");
        let build_id = uuid::Uuid::new_v4().to_string();
        storage
            .insert_build(&build_id, &project_id, "success")
            .await
            .expect("build");
        let firmware = Firmware {
            id: uuid::Uuid::new_v4().to_string(),
            project_id,
            build_id,
            project: "test".into(),
            target: "esp32s3".into(),
            version: "1.1.0".into(),
            filename: "firmware-test.bin".into(),
            path: "firmware-test.bin".into(),
            file_size: 4096,
            sha256: "a".repeat(64),
            status: "ready".into(),
            created_at: now(),
        };
        storage.insert_firmware(&firmware).await.expect("firmware");

        let first = uuid::Uuid::new_v4().to_string();
        storage
            .create_deployment(&first, device_id, &firmware)
            .await
            .expect("first deployment");
        let second = uuid::Uuid::new_v4().to_string();
        storage
            .create_deployment(&second, device_id, &firmware)
            .await
            .expect("replacement deployment");
        let first_status: String = sqlx::query_scalar("SELECT status FROM deployments WHERE id=$1")
            .bind(&first)
            .fetch_one(storage.pool())
            .await
            .expect("superseded deployment");
        assert_eq!(first_status, "superseded");

        let cancelled = storage.cancel_update(device_id).await.expect("cancel update");
        assert!(cancelled.desired_firmware_id.is_none());
        assert_eq!(cancelled.ota_status, "idle");
        let second_status: String = sqlx::query_scalar("SELECT status FROM deployments WHERE id=$1")
            .bind(&second)
            .fetch_one(storage.pool())
            .await
            .expect("cancelled deployment");
        assert_eq!(second_status, "cancelled");
        storage.cancel_update(device_id).await.expect("idempotent cancel");
        let error = storage
            .update_ota_status(device_id, "success", 100, Some("9.9.9"), None)
            .await
            .expect_err("a callback cannot invent a version without a deployment");
        assert_eq!(error.code, "NO_MATCHING_DEPLOYMENT");

        let third = uuid::Uuid::new_v4().to_string();
        storage
            .create_deployment(&third, device_id, &firmware)
            .await
            .expect("new deployment");
        let error = storage
            .update_ota_status(device_id, "success", 100, Some("9.9.9"), None)
            .await
            .expect_err("incorrect boot version must not complete deployment");
        assert_eq!(error.code, "OTA_VERSION_MISMATCH");
        storage
            .upsert_device(device_id, "Test", "esp32s3", "1.1.0", "127.0.0.1")
            .await
            .expect("new image registered after reboot");
        let device = storage.get_device(device_id).await.expect("device");
        assert!(device.desired_firmware_id.is_none());
        assert_eq!(device.current_version, "1.1.0");
        assert_eq!(device.ota_status, "success");
        assert_eq!(device.ota_progress, 100);
        storage
            .update_ota_status(device_id, "success", 100, Some("1.1.0"), None)
            .await
            .expect("success callback after registration is idempotent");
        let third_status: String = sqlx::query_scalar("SELECT status FROM deployments WHERE id=$1")
            .bind(&third)
            .fetch_one(storage.pool())
            .await
            .expect("completed deployment");
        assert_eq!(third_status, "success");
        assert_eq!(storage.begin_firmware_deletion(&firmware.id).await.expect_err("running image protected").code, "FIRMWARE_IN_USE");
        assert_eq!(storage.get_firmware(&firmware.id).await.expect("failed reservation rolls back").status, "ready");

        // A later rollback/recovery boot into the old image must not cause an
        // image that already reached the reboot stage to be offered forever.
        storage
            .heartbeat_device(device_id, "1.0.0", "online", "127.0.0.1")
            .await
            .expect("old image booted");
        let fourth = uuid::Uuid::new_v4().to_string();
        storage
            .create_deployment(&fourth, device_id, &firmware)
            .await
            .expect("deployment before failed boot");
        storage
            .update_ota_status(device_id, "rebooting", 100, None, None)
            .await
            .expect("device rebooting");
        storage
            .upsert_device(device_id, "Test", "esp32s3", "1.0.0", "127.0.0.1")
            .await
            .expect("previous version re-registered");
        let recovered = storage.get_device(device_id).await.expect("recovered device");
        assert!(recovered.desired_firmware_id.is_none());
        assert_eq!(recovered.ota_status, "failed");
        assert!(recovered.ota_error.is_some());
        let fourth_status: String = sqlx::query_scalar("SELECT status FROM deployments WHERE id=$1")
            .bind(&fourth)
            .fetch_one(storage.pool())
            .await
            .expect("failed deployment");
        assert_eq!(fourth_status, "failed");
        let error = storage
            .update_ota_status(device_id, "success", 100, Some("1.0.0"), None)
            .await
            .expect_err("an old image must not erase the failed rollback result");
        assert_eq!(error.code, "NO_MATCHING_DEPLOYMENT");
        // Cancelled, superseded, successful and failed historical attempts no
        // longer prevent deletion once the device runs another version.
        storage.begin_firmware_deletion(&firmware.id).await.expect("only history remains");
        assert!(storage.get_firmware(&firmware.id).await.is_err());
        assert_eq!(storage.list_firmware().await.expect("list pending cleanup")[0].status, "deleting");
        let stale_deploy = storage.create_deployment(&uuid::Uuid::new_v4().to_string(), device_id, &firmware).await.expect_err("stale handler cannot deploy a deleting file");
        assert_eq!(stale_deploy.code, "FIRMWARE_NOT_READY");
        storage.begin_firmware_deletion(&firmware.id).await.expect("interrupted cleanup can be retried");
        storage.finish_firmware_deletion(&firmware.id).await.expect("finish deletion");
        assert!(storage.list_firmware().await.expect("list").is_empty());
        assert!(storage.latest_firmware("esp32s3").await.is_err());
        let history: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE firmware_id=$1").bind(&firmware.id).fetch_one(storage.pool()).await.expect("history");
        assert_eq!(history, 4);
    }
    #[tokio::test]
    #[ignore = "Requires disposable PostgreSQL via TEST_DATABASE_URL"]
    async fn postgres_integration() {
        exercise(&std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL")).await;
    }
}
