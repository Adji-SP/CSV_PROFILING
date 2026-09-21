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
        sqlx::query_as::<_, Firmware>("SELECT * FROM firmware WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Firmware not found"))
    }

    pub async fn list_firmware(&self) -> ApiResult<Vec<Firmware>> {
        Ok(
            sqlx::query_as::<_, Firmware>("SELECT * FROM firmware ORDER BY created_at DESC")
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

    pub async fn firmware_reference_count(&self, id: &str) -> ApiResult<i64> {
        let device_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE desired_firmware_id=$1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        let deployment_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE firmware_id=$1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(device_count + deployment_count)
    }

    pub async fn delete_firmware(&self, id: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM firmware WHERE id=$1")
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
        self.get_device(device_id).await
    }

    pub async fn create_deployment(
        &self,
        deployment_id: &str,
        device_id: &str,
        firmware: &Firmware,
    ) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
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

    pub async fn update_ota_status(
        &self,
        device_id: &str,
        status: &str,
        progress: i64,
        current_version: Option<&str>,
        error: Option<&str>,
    ) -> ApiResult<Device> {
        let mut tx = self.pool.begin().await?;
        let desired_version: Option<String> =
            sqlx::query_scalar("SELECT desired_version FROM devices WHERE device_id=$1")
                .bind(device_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();

        let resolved_version = current_version.or(desired_version.as_deref());
        if status == "success" {
            sqlx::query(
                r#"UPDATE devices SET ota_status='success',ota_progress=100,
                ota_error=NULL,current_version=COALESCE($1,current_version),
                desired_version=NULL,desired_firmware_id=NULL,last_seen=$2 WHERE device_id=$3"#,
            )
            .bind(resolved_version)
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

        sqlx::query(r#"UPDATE deployments SET status=$1,progress=$2,error=$3,updated_at=$4
            WHERE id=(SELECT id FROM deployments WHERE device_id=$5 ORDER BY created_at DESC LIMIT 1)"#)
            .bind(status)
            .bind(progress)
            .bind(error)
            .bind(now())
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
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
        assert!(
            storage
                .firmware_reference_count(&firmware.id)
                .await
                .expect("references")
                > 0
        );
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
        storage
            .delete_firmware(&firmware.id)
            .await
            .expect("cleanup firmware");
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
    #[ignore = "Requires disposable PostgreSQL via TEST_DATABASE_URL"]
    async fn postgres_integration() {
        exercise(&std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL")).await;
    }
}
