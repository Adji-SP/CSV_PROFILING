use std::{str::FromStr, time::Duration};

use chrono::Utc;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};

use crate::{
    errors::{ApiError, ApiResult},
    models::{Build, Device, Firmware, Project},
};

#[derive(Debug, Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub async fn connect(database_url: &str) -> ApiResult<Self> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(|error| ApiError::internal("DATABASE_CONFIG_ERROR", error.to_string()))?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePool::connect_with(options).await?;
        let storage = Self { pool };
        storage.migrate().await?;
        Ok(storage)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn migrate(&self) -> ApiResult<()> {
        let statements = [
            r#"CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                target TEXT NOT NULL,
                version TEXT NOT NULL,
                path TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL
            )"#,
            r#"CREATE TABLE IF NOT EXISTS builds (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                status TEXT NOT NULL,
                logs_text TEXT NOT NULL DEFAULT '',
                exit_code INTEGER,
                error TEXT,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
            )"#,
            r#"CREATE TABLE IF NOT EXISTS firmware (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                build_id TEXT NOT NULL UNIQUE,
                project TEXT NOT NULL,
                target TEXT NOT NULL,
                version TEXT NOT NULL,
                filename TEXT NOT NULL UNIQUE,
                path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE RESTRICT,
                FOREIGN KEY(build_id) REFERENCES builds(id) ON DELETE RESTRICT
            )"#,
            r#"CREATE TABLE IF NOT EXISTS devices (
                device_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                chip TEXT NOT NULL,
                ip_address TEXT NOT NULL,
                current_version TEXT NOT NULL,
                desired_version TEXT,
                desired_firmware_id TEXT,
                status TEXT NOT NULL,
                ota_status TEXT NOT NULL,
                ota_progress INTEGER NOT NULL DEFAULT 0,
                ota_error TEXT,
                last_seen TEXT NOT NULL,
                FOREIGN KEY(desired_firmware_id) REFERENCES firmware(id) ON DELETE RESTRICT
            )"#,
            r#"CREATE TABLE IF NOT EXISTS deployments (
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL,
                firmware_id TEXT NOT NULL,
                status TEXT NOT NULL,
                progress INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(device_id) REFERENCES devices(device_id) ON DELETE CASCADE,
                FOREIGN KEY(firmware_id) REFERENCES firmware(id) ON DELETE RESTRICT
            )"#,
            "CREATE INDEX IF NOT EXISTS idx_builds_project ON builds(project_id)",
            "CREATE INDEX IF NOT EXISTS idx_firmware_target_created ON firmware(target, created_at DESC)",
            "CREATE INDEX IF NOT EXISTS idx_deployments_device ON deployments(device_id, created_at DESC)",
        ];
        for statement in statements {
            sqlx::query(statement).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn insert_project(&self, project: &Project) -> ApiResult<()> {
        sqlx::query(
            "INSERT INTO projects (id,name,target,version,path,status,created_at) VALUES (?,?,?,?,?,?,?)",
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
        sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE id = ?")
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
        sqlx::query("UPDATE projects SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_build(&self, id: &str, project_id: &str, status: &str) -> ApiResult<()> {
        sqlx::query("INSERT INTO builds (id,project_id,status,created_at) VALUES (?,?,?,?)")
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
            FROM builds b JOIN projects p ON p.id=b.project_id WHERE b.id=?"#;
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
        sqlx::query("UPDATE builds SET status='building',started_at=? WHERE id=?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn append_build_log(&self, id: &str, line: &str) -> ApiResult<()> {
        let safe_line = line.replace('\0', "");
        sqlx::query("UPDATE builds SET logs_text = logs_text || ? || char(10) WHERE id=?")
            .bind(safe_line)
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
        sqlx::query("UPDATE builds SET status=?,exit_code=?,error=?,finished_at=? WHERE id=?")
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
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"#)
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
        sqlx::query_as::<_, Firmware>("SELECT * FROM firmware WHERE id=?")
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
            "SELECT * FROM firmware WHERE target=? AND status='ready' ORDER BY created_at DESC LIMIT 1",
        )
        .bind(target)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("No ready firmware exists for this target"))
    }

    pub async fn firmware_reference_count(&self, id: &str) -> ApiResult<i64> {
        let device_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE desired_firmware_id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        let deployment_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE firmware_id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(device_count + deployment_count)
    }

    pub async fn delete_firmware(&self, id: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM firmware WHERE id=?")
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
            VALUES (?,?,?,?,?,'online','idle',0,?)
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
        sqlx::query_as::<_, Device>("SELECT * FROM devices WHERE device_id=?")
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
            current_version=?,status=?,ip_address=?,last_seen=? WHERE device_id=?"#,
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
            VALUES (?,?,?,'pending',0,?,?)"#,
        )
        .bind(deployment_id)
        .bind(device_id)
        .bind(&firmware.id)
        .bind(now())
        .bind(now())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE devices SET desired_version=?,desired_firmware_id=?,
            ota_status='pending',ota_progress=0,ota_error=NULL WHERE device_id=?"#,
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
            sqlx::query_scalar("SELECT desired_version FROM devices WHERE device_id=?")
                .bind(device_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();

        let resolved_version = current_version.or(desired_version.as_deref());
        if status == "success" {
            sqlx::query(
                r#"UPDATE devices SET ota_status='success',ota_progress=100,
                ota_error=NULL,current_version=COALESCE(?,current_version),
                desired_version=NULL,desired_firmware_id=NULL,last_seen=? WHERE device_id=?"#,
            )
            .bind(resolved_version)
            .bind(now())
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                r#"UPDATE devices SET ota_status=?,ota_progress=?,ota_error=?,
                current_version=COALESCE(?,current_version),last_seen=? WHERE device_id=?"#,
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

        sqlx::query(r#"UPDATE deployments SET status=?,progress=?,error=?,updated_at=?
            WHERE id=(SELECT id FROM deployments WHERE device_id=? ORDER BY created_at DESC LIMIT 1)"#)
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
