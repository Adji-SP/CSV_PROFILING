use super::{protocol, repository::Repository};
use crate::{
    errors::{ApiError, ApiResult},
    storage::Storage,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, broadcast, watch};

#[derive(Clone)]
pub struct Settings {
    pub enabled: bool,
    pub prefix: String,
    pub max_bytes: usize,
    pub timeout_seconds: i64,
}
impl Settings {
    pub fn load() -> ApiResult<Self> {
        let parse =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
        let enabled = parse("TINYML_MQTT_ENABLED", "true")
            .parse()
            .map_err(|_| protocol::invalid("Invalid TINYML_MQTT_ENABLED"))?;
        let prefix = parse("TINYML_MQTT_TOPIC_PREFIX", "tinyml/v1");
        if prefix.split('/').any(|s| !protocol::valid_id(s)) {
            return Err(protocol::invalid("Invalid TinyML topic prefix"));
        }
        let max_bytes = parse("TINYML_MAX_MESSAGE_BYTES", "65536")
            .parse()
            .map_err(|_| protocol::invalid("Invalid TinyML payload limit"))?;
        let timeout_seconds = parse("TINYML_RUN_TIMEOUT_SECONDS", "300")
            .parse()
            .map_err(|_| protocol::invalid("Invalid TinyML timeout"))?;
        if !(1024..=1_048_576).contains(&max_bytes) || timeout_seconds < 1 {
            return Err(protocol::invalid("Invalid TinyML size/timeout limits"));
        }
        Ok(Self {
            enabled,
            prefix,
            max_bytes,
            timeout_seconds,
        })
    }
}
pub struct TinyMl {
    pub repo: Repository,
    pub settings: Settings,
    pub events: broadcast::Sender<Value>,
    ingestion: Mutex<()>,
    aggregation: Mutex<()>,
}
impl TinyMl {
    pub fn new(storage: &Storage, settings: Settings) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            repo: Repository {
                pool: storage.pool().clone(),
            },
            settings,
            events,
            ingestion: Mutex::new(()),
            aggregation: Mutex::new(()),
        })
    }
    pub async fn ingest(&self, topic: &str, bytes: &[u8]) -> ApiResult<bool> {
        let parsed = protocol::parse(topic, bytes, &self.settings.prefix, self.settings.max_bytes)?;
        let payload = std::str::from_utf8(bytes).map_err(|_| protocol::invalid("Invalid UTF-8"))?;
        let _guard = self.ingestion.lock().await;
        let outcome = self.repo.ingest(topic, &parsed, payload).await?;
        if let Some((run, event)) = outcome {
            let _=self.events.send(json!({"type":event,"device_id":parsed.envelope.device_id,"run_id":run.as_ref().map(|r|&r.id),"data":run.as_ref().map(|r|r.view())}));
            return Ok(true);
        }
        Ok(false)
    }
    pub async fn metrics(&self, id: &str) -> ApiResult<Value> {
        // Expensive work is on-demand and serialized, never performed per sample.
        let _guard = self.aggregation.lock().await;
        serde_json::to_value(self.repo.aggregate(id).await?)
            .map_err(|_| ApiError::internal("TINYML_REPORT", "Cannot serialize metrics"))
    }
    pub async fn maintain(&self) -> ApiResult<()> {
        let now = crate::storage::now();
        let cutoff = (chrono::Utc::now()
            - chrono::Duration::seconds(self.settings.timeout_seconds))
        .to_rfc3339();
        sqlx::query("UPDATE tinyml_runs SET status='completing',final_status='timed_out',completed_at_server=$1 WHERE status IN ('created','running') AND last_received_at<$2")
            .bind(now).bind(cutoff).execute(&self.repo.pool).await?;
        let pending:Vec<String>=sqlx::query_scalar("SELECT id FROM tinyml_runs WHERE status='completing' ORDER BY completed_at_server LIMIT 10").fetch_all(&self.repo.pool).await?;
        for id in pending {
            let _guard = self.aggregation.lock().await;
            if self.repo.finalize(&id).await? {
                let run = self.repo.run(&id).await?;
                for event in ["tinyml.run.completed", "tinyml.report.ready"] {
                    let _=self.events.send(json!({"type":event,"device_id":run.device_id,"run_id":id,"data":run.view()}));
                }
            }
        }
        Ok(())
    }
    pub fn start(self: &Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! {
                    _=shutdown.changed()=>break,
                    _=interval.tick()=>if let Err(error)=service.maintain().await {tracing::error!(code=error.code,"TinyML maintenance failed; pending reports will retry");}
                }
            }
        });
    }
}
