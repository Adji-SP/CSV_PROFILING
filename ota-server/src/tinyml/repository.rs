use super::{
    models::*,
    protocol::{self, Data, Parsed},
};
use crate::errors::{ApiError, ApiResult};
use serde_json::{Value, json};
use sqlx::{AnyPool, Row};

#[derive(Clone)]
pub struct Repository {
    pub pool: AnyPool,
}
impl Repository {
    pub async fn run(&self, id: &str) -> ApiResult<Run> {
        sqlx::query_as("SELECT * FROM tinyml_runs WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("TinyML run not found; use the internal run id"))
    }
    pub async fn ingest(
        &self,
        topic: &str,
        parsed: &Parsed,
        payload: &str,
    ) -> ApiResult<Option<(Option<Run>, String)>> {
        let e = &parsed.envelope;
        let now = crate::storage::now();
        let mut tx = self.pool.begin().await?;
        // All message, run, result and counter writes commit together.
        let registered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE device_id=$1")
            .bind(&e.device_id)
            .fetch_one(&mut *tx)
            .await?;
        if registered == 0 {
            return Err(protocol::invalid(
                "Device must register with the OTA service first",
            ));
        }
        let prior: Option<String> =
            sqlx::query_scalar("SELECT payload_json FROM tinyml_messages WHERE message_id=$1")
                .bind(&e.message_id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(prior) = prior {
            if json_value(&prior) != json_value(payload) {
                return Err(protocol::invalid(
                    "message_id already belongs to another payload",
                ));
            }
            return Ok(None);
        }
        let run_key = if e.message_type != protocol::MessageType::Status {
            let proposed = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO tinyml_runs(id,device_id,run_id,status,started_at_server,last_received_at) VALUES($1,$2,$3,'created',$4,$4) ON CONFLICT(device_id,run_id) DO NOTHING")
                .bind(&proposed).bind(&e.device_id).bind(&e.run_id).bind(&now).execute(&mut *tx).await?;
            let id: String =
                sqlx::query_scalar("SELECT id FROM tinyml_runs WHERE device_id=$1 AND run_id=$2")
                    .bind(&e.device_id)
                    .bind(&e.run_id)
                    .fetch_one(&mut *tx)
                    .await?;
            // Serialize this run across connections/instances (SQLite serializes writers).
            sqlx::query("UPDATE tinyml_runs SET id=id WHERE id=$1")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            Some(id)
        } else {
            None
        };
        let inserted=sqlx::query("INSERT INTO tinyml_messages(message_id,device_id,run_key,message_type,topic,received_at,payload_json) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(message_id) DO NOTHING")
            .bind(&e.message_id).bind(&e.device_id).bind(&run_key).bind(format!("{:?}",e.message_type)).bind(topic).bind(&now).bind(payload).execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            let prior: String =
                sqlx::query_scalar("SELECT payload_json FROM tinyml_messages WHERE message_id=$1")
                    .bind(&e.message_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if json_value(&prior) != json_value(payload) {
                return Err(protocol::invalid(
                    "message_id already belongs to another payload",
                ));
            }
            return Ok(None);
        }
        if let Some(id) = &run_key {
            let state: String = sqlx::query_scalar("SELECT status FROM tinyml_runs WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
            if !["created", "running"].contains(&state.as_str()) {
                if matches!(parsed.data, Data::Complete(_)) {
                    tx.commit().await?;
                    return Ok(None);
                }
                return Err(ApiError::conflict(
                    "TINYML_RUN_CLOSED",
                    "Run is closed; late messages cannot change its report",
                ));
            }
            if matches!(parsed.data, Data::Start(_)) && state != "created" {
                return Err(ApiError::conflict(
                    "TINYML_RUN_STARTED",
                    "Run has already started",
                ));
            }
            sqlx::query("UPDATE tinyml_runs SET last_received_at=$1 WHERE id=$2")
                .bind(&now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let event = match &parsed.data {
            Data::Start(s) => {
                sqlx::query("UPDATE tinyml_runs SET status='running',run_name=$1,mode=$2,started_at_device=$3,start_json=$4,other_json=$5 WHERE id=$6")
                    .bind(&s.run_name).bind(&s.mode).bind(&e.timestamp).bind(e.data.to_string()).bind(e.other.to_string()).bind(&run_key).execute(&mut *tx).await?;
                "tinyml.run.started"
            }
            Data::Results(results) => {
                for (index, r) in results.iter().enumerate() {
                    let timing = r.timing.clone().unwrap_or_default();
                    let system = r.system.clone().unwrap_or_default();
                    let other = if matches!(e.message_type, protocol::MessageType::InferenceBatch) {
                        r.other.clone()
                    } else {
                        e.other.clone()
                    };
                    let sample = match &r.sample_id {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        v => Some(v.to_string()),
                    };
                    let integer = |n: Option<u64>| -> ApiResult<Option<i64>> {
                        n.map(|v| {
                            i64::try_from(v).map_err(|_| {
                                protocol::invalid("Memory measurement exceeds signed 64-bit range")
                            })
                        })
                        .transpose()
                    };
                    sqlx::query("INSERT INTO tinyml_results(id,run_key,message_id,batch_index,sequence,sample_id,device_timestamp,server_timestamp,actual_class,predicted_class,confidence,correct,inference_us,total_pipeline_us,preprocessing_us,postprocessing_us,free_heap_bytes,minimum_free_heap_bytes,chip_temperature_c,other_json,data_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)")
                        .bind(uuid::Uuid::new_v4().to_string()).bind(&run_key).bind(&e.message_id).bind(index as i64).bind(e.sequence).bind(sample)
                        .bind(&e.timestamp).bind(&now).bind(&r.actual_class).bind(&r.predicted_class).bind(r.confidence).bind(r.correct.map(i64::from))
                        .bind(timing.inference_us).bind(timing.total_pipeline_us).bind(timing.preprocessing_us).bind(timing.postprocessing_us)
                        .bind(integer(system.free_heap_bytes)?).bind(integer(system.minimum_free_heap_bytes)?).bind(system.chip_temperature_c)
                        .bind(other.to_string()).bind(serde_json::to_string(r).map_err(|_|protocol::invalid("Invalid result"))?).execute(&mut *tx).await?;
                    sqlx::query("UPDATE tinyml_runs SET samples=samples+1,labelled=labelled+$1,correct_count=correct_count+$2,confidence_sum=confidence_sum+$3,confidence_count=confidence_count+$4,latency_sum_us=latency_sum_us+$5,latency_count=latency_count+$6 WHERE id=$7")
                        .bind(i64::from(r.correct.is_some())).bind(i64::from(r.correct==Some(true))).bind(r.confidence.unwrap_or(0.0)).bind(i64::from(r.confidence.is_some()))
                        .bind(timing.inference_us.unwrap_or(0.0)).bind(i64::from(timing.inference_us.is_some())).bind(&run_key).execute(&mut *tx).await?;
                }
                "tinyml.inference"
            }
            Data::Snapshot => {
                sqlx::query("INSERT INTO tinyml_metric_snapshots(id,run_key,message_id,sequence,device_timestamp,server_timestamp,data_json,other_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                    .bind(uuid::Uuid::new_v4().to_string()).bind(&run_key).bind(&e.message_id).bind(e.sequence).bind(&e.timestamp).bind(&now).bind(e.data.to_string()).bind(e.other.to_string()).execute(&mut *tx).await?;
                "tinyml.metrics.updated"
            }
            Data::Complete(c) => {
                sqlx::query("UPDATE tinyml_runs SET status='completing',final_status=$1,completed_at_device=$2,completed_at_server=$3,complete_json=$4 WHERE id=$5")
                    .bind(&c.status).bind(&e.timestamp).bind(&now).bind(json!({"data":e.data,"other":e.other}).to_string()).bind(&run_key).execute(&mut *tx).await?;
                "tinyml.run.completing"
            }
            Data::Event | Data::Status => {
                sqlx::query("INSERT INTO tinyml_events(id,run_key,device_id,message_id,message_type,device_timestamp,server_timestamp,data_json,other_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                    .bind(uuid::Uuid::new_v4().to_string()).bind(&run_key).bind(&e.device_id).bind(&e.message_id).bind(format!("{:?}",e.message_type)).bind(&e.timestamp).bind(&now).bind(e.data.to_string()).bind(e.other.to_string()).execute(&mut *tx).await?;
                "tinyml.event"
            }
        };
        tx.commit().await?;
        let run = match run_key {
            Some(id) => Some(self.run(&id).await?),
            None => None,
        };
        Ok(Some((run, event.into())))
    }
    pub async fn runs(&self, f: &Filters) -> ApiResult<Value> {
        let (page, size) = f.pagination()?;
        let condition = "WHERE (CAST($1 AS TEXT) IS NULL OR device_id=$1) AND (CAST($2 AS TEXT) IS NULL OR status=$2)";
        let total: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM tinyml_runs {condition}"))
                .bind(&f.device_id)
                .bind(&f.status)
                .fetch_one(&self.pool)
                .await?;
        let rows:Vec<Run>=sqlx::query_as(&format!("SELECT * FROM tinyml_runs {condition} ORDER BY started_at_server DESC,id DESC LIMIT $3 OFFSET $4"))
            .bind(&f.device_id).bind(&f.status).bind(size).bind((page-1)*size).fetch_all(&self.pool).await?;
        Ok(
            json!({"items":rows.iter().map(Run::view).collect::<Vec<_>>(),"total":total,"page":page,"page_size":size}),
        )
    }
    pub async fn results(&self, id: &str, f: &Filters) -> ApiResult<Value> {
        self.run(id).await?;
        let (page, size) = f.pagination()?;
        let condition = "WHERE run_key=$1 AND (CAST($2 AS TEXT) IS NULL OR actual_class=$2) AND (CAST($3 AS TEXT) IS NULL OR predicted_class=$3) AND (CAST($4 AS BIGINT) IS NULL OR correct=$4) AND (CAST($5 AS DOUBLE PRECISION) IS NULL OR confidence >= $5) AND (CAST($6 AS DOUBLE PRECISION) IS NULL OR confidence <= $6) AND (CAST($7 AS TEXT) IS NULL OR LOWER(COALESCE(sample_id,'') || ' ' || COALESCE(actual_class,'') || ' ' || COALESCE(predicted_class,'') || ' ' || other_json) LIKE $7)";
        let search = f.search.as_ref().map(|s| format!("%{}%", s.to_lowercase()));
        let total: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM tinyml_results {condition}"))
                .bind(id)
                .bind(&f.actual_class)
                .bind(&f.predicted_class)
                .bind(f.correct.map(i64::from))
                .bind(f.min_confidence)
                .bind(f.max_confidence)
                .bind(&search)
                .fetch_one(&self.pool)
                .await?;
        let sort = match f.sort.as_deref().unwrap_or("time") {
            "time" => "server_timestamp",
            "sequence" => "sequence",
            "confidence" => "confidence",
            "inference" => "inference_us",
            "sample" => "sample_id",
            _ => return Err(protocol::invalid("Unknown result sort")),
        };
        let order = match f.order.as_deref().unwrap_or("desc") {
            "asc" => "ASC",
            "desc" => "DESC",
            _ => return Err(protocol::invalid("order must be asc or desc")),
        };
        let rows:Vec<ResultRow>=sqlx::query_as(&format!("SELECT * FROM tinyml_results {condition} ORDER BY {sort} {order},id {order} LIMIT $8 OFFSET $9"))
            .bind(id).bind(&f.actual_class).bind(&f.predicted_class).bind(f.correct.map(i64::from)).bind(f.min_confidence).bind(f.max_confidence).bind(&search).bind(size).bind((page-1)*size).fetch_all(&self.pool).await?;
        Ok(
            json!({"items":rows.iter().map(ResultRow::view).collect::<Vec<_>>(),"total":total,"page":page,"page_size":size}),
        )
    }
    pub async fn report(&self, id: &str) -> ApiResult<Value> {
        let text: Option<String> =
            sqlx::query_scalar("SELECT report_json FROM tinyml_reports WHERE run_key=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        text.map(|s| json_value(&s))
            .ok_or_else(|| ApiError::not_found("Report is not ready yet"))
    }
    pub async fn overview(&self) -> ApiResult<Value> {
        let r=sqlx::query("SELECT COUNT(*) AS runs,CAST(COALESCE(SUM(CASE WHEN status IN ('running','created','completing') THEN 1 ELSE 0 END),0) AS BIGINT) AS active,CAST(COALESCE(SUM(CASE WHEN status='completed' THEN 1 ELSE 0 END),0) AS BIGINT) AS completed,CAST(COALESCE(SUM(samples),0) AS BIGINT) AS samples,CAST(COALESCE(SUM(labelled),0) AS BIGINT) AS labelled,CAST(COALESCE(SUM(correct_count),0) AS BIGINT) AS correct,COALESCE(SUM(latency_sum_us),0.0) AS latency,CAST(COALESCE(SUM(latency_count),0) AS BIGINT) AS latency_count FROM tinyml_runs").fetch_one(&self.pool).await?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(90)).to_rfc3339();
        let devices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE last_seen >= $1")
            .bind(cutoff)
            .fetch_one(&self.pool)
            .await?;
        Ok(
            json!({"connected_devices":devices,"runs":r.try_get::<i64,_>("runs")?,"active_runs":r.try_get::<i64,_>("active")?,"completed_runs":r.try_get::<i64,_>("completed")?,"total_inferences":r.try_get::<i64,_>("samples")?,"accuracy":ratio(r.try_get::<i64,_>("correct")? as f64,r.try_get("labelled")?),"mean_inference_ms":ratio(r.try_get::<f64,_>("latency")?/1000.0,r.try_get("latency_count")?)}),
        )
    }
    pub async fn observations(&self, id: &str, kind: &str) -> ApiResult<Vec<Value>> {
        // Table identifiers are a closed enum chosen by the server.
        let table = if kind == "snapshots" {
            "tinyml_metric_snapshots"
        } else {
            "tinyml_events"
        };
        let rows=sqlx::query(&format!("SELECT device_timestamp,server_timestamp,data_json,other_json FROM {table} WHERE run_key=$1 ORDER BY server_timestamp DESC,id DESC LIMIT 500"))
            .bind(id).fetch_all(&self.pool).await?;
        rows.iter().map(|r|Ok(json!({"device_timestamp":r.try_get::<Option<String>,_>("device_timestamp")?,"server_timestamp":r.try_get::<String,_>("server_timestamp")?,"data":json_value(&r.try_get::<String,_>("data_json")?),"other":json_value(&r.try_get::<String,_>("other_json")?)}))).collect()
    }
}
