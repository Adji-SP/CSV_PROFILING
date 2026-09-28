use super::{metrics::Accumulator, models::*, repository::Repository};
use crate::errors::{ApiError, ApiResult};
use sqlx::{Acquire, Row};

impl Repository {
    pub async fn aggregate(&self, id: &str) -> ApiResult<super::metrics::TinyMlReport> {
        let mut connection = self.pool.acquire().await?;
        let postgres = connection.backend_name() == "PostgreSQL";
        let mut tx = connection.begin().await?;
        if postgres {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx)
                .await?;
        }
        let run: Run = sqlx::query_as("SELECT * FROM tinyml_runs WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::not_found("Run not found"))?;
        let mut accumulator = Accumulator::new(&run);
        // Use one read transaction for a consistent active-run snapshot.
        // Terminal runs are immutable; active metrics are explicitly provisional.
        let mut cursor = String::new();
        let mut timestamp = String::new();
        loop {
            let rows:Vec<ResultRow>=sqlx::query_as("SELECT * FROM tinyml_results WHERE run_key=$1 AND (server_timestamp>$2 OR (server_timestamp=$2 AND id>$3)) ORDER BY server_timestamp,id LIMIT 1000")
                .bind(id).bind(&timestamp).bind(&cursor).fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            cursor = rows.last().map(|r| r.id.clone()).unwrap_or_default();
            timestamp = rows
                .last()
                .map(|r| r.server_timestamp.clone())
                .unwrap_or_default();
            accumulator = tokio::task::spawn_blocking(move || {
                for row in rows {
                    accumulator.add(&row);
                }
                accumulator
            })
            .await
            .map_err(|_| ApiError::internal("TINYML_REPORT", "Aggregation task failed"))?;
        }
        let mut cursor = String::new();
        let mut timestamp = String::new();
        loop {
            let rows=sqlx::query("SELECT id,server_timestamp,data_json,other_json FROM tinyml_metric_snapshots WHERE run_key=$1 AND (server_timestamp>$2 OR (server_timestamp=$2 AND id>$3)) ORDER BY server_timestamp,id LIMIT 500").bind(id).bind(&timestamp).bind(&cursor).fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                cursor = row.try_get("id")?;
                timestamp = row.try_get("server_timestamp")?;
                accumulator.snapshot(
                    &json_value(&row.try_get::<String, _>("data_json")?),
                    &json_value(&row.try_get::<String, _>("other_json")?),
                );
            }
        }
        tx.commit().await?;
        tokio::task::spawn_blocking(move || accumulator.finish(&run))
            .await
            .map_err(|_| ApiError::internal("TINYML_REPORT", "Aggregation task failed"))
    }
    pub async fn finalize(&self, id: &str) -> ApiResult<bool> {
        let run = self.run(id).await?;
        if run.status != "completing" {
            return Ok(false);
        }
        let report = self.aggregate(id).await?;
        let encoded = serde_json::to_string(&report)
            .map_err(|_| ApiError::internal("TINYML_REPORT", "Cannot serialize report"))?;
        let mut tx = self.pool.begin().await?;
        let inserted=sqlx::query("INSERT INTO tinyml_reports(run_key,generated_at,report_version,report_json) VALUES($1,$2,'1.0',$3) ON CONFLICT(run_key) DO NOTHING")
            .bind(id).bind(&report.generated_at).bind(encoded).execute(&mut *tx).await?;
        sqlx::query("UPDATE tinyml_runs SET status=COALESCE(final_status,'completed') WHERE id=$1 AND status='completing'").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(inserted.rows_affected() > 0)
    }
}
