use crate::{
    errors::{ApiError, ApiResult},
    state::AppState,
    tinyml::models::{Filters, ResultRow},
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{
        Path, Query, Request, State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Acquire;
use std::time::Duration;

pub fn router(state: AppState) -> Router<AppState> {
    let secured = Router::new()
        .route("/api/tinyml/status", get(status))
        .route("/api/tinyml/devices", get(devices))
        .route("/api/tinyml/runs", get(runs))
        .route("/api/tinyml/runs/{id}", get(run))
        .route("/api/tinyml/runs/{id}/results", get(results))
        .route("/api/tinyml/runs/{id}/metrics", get(metrics))
        .route("/api/tinyml/runs/{id}/report", get(report))
        .route("/api/tinyml/runs/{id}/report/json", get(report_download))
        .route("/api/tinyml/runs/{id}/results.csv", get(csv))
        .route("/api/tinyml/runs/{id}/other-fields", get(other_fields))
        .route("/api/tinyml/runs/{id}/snapshots", get(snapshots))
        .route("/api/tinyml/runs/{id}/events", get(events))
        .route_layer(middleware::from_fn_with_state(state, authenticate));
    secured.route("/api/tinyml/ws", get(stream))
}
async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if !crate::console::authorized(&state.console, token) {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "Enter the viewer access token",
        )
        .into_response();
    }
    next.run(request).await
}
async fn status(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({"schema_version":"1.0","mqtt_enabled":s.tinyml.settings.enabled,"topic_prefix":s.tinyml.settings.prefix,"overview":s.tinyml.repo.overview().await?}),
    ))
}
async fn devices(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(
        serde_json::to_value(s.storage.list_devices().await?)
            .map_err(|_| ApiError::internal("SERIALIZE", "Cannot serialize devices"))?,
    ))
}
async fn runs(State(s): State<AppState>, Query(f): Query<Filters>) -> ApiResult<Json<Value>> {
    Ok(Json(s.tinyml.repo.runs(&f).await?))
}
async fn run(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(s.tinyml.repo.run(&id).await?.view()))
}
async fn results(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(f): Query<Filters>,
) -> ApiResult<Json<Value>> {
    Ok(Json(s.tinyml.repo.results(&id, &f).await?))
}
async fn metrics(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(s.tinyml.metrics(&id).await?))
}
async fn report(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.tinyml.repo.run(&id).await?;
    Ok(Json(s.tinyml.repo.report(&id).await?))
}
async fn report_download(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let value = s.tinyml.repo.report(&id).await?;
    Ok((
        [(
            "content-disposition",
            "attachment; filename=\"tinyml-report.json\"",
        )],
        Json(value),
    )
        .into_response())
}
async fn other_fields(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let r = match s.tinyml.repo.report(&id).await {
        Ok(r) => r,
        Err(_) => s.tinyml.metrics(&id).await?,
    };
    Ok(Json(r.get("other").cloned().unwrap_or(Value::Null)))
}
async fn snapshots(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.tinyml.repo.run(&id).await?;
    Ok(Json(
        json!({"items":s.tinyml.repo.observations(&id,"snapshots").await?,"limit":500}),
    ))
}
async fn events(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.tinyml.repo.run(&id).await?;
    Ok(Json(
        json!({"items":s.tinyml.repo.observations(&id,"events").await?,"limit":500}),
    ))
}

pub(crate) fn csv_cell(value: &str) -> String {
    // Quote CSV and neutralize formula-leading text for spreadsheet consumers.
    let prefix = if value
        .trim_start()
        .starts_with(['=', '+', '-', '@', '\t', '\r'])
    {
        "'"
    } else {
        ""
    };
    format!("\"{prefix}{}\"", value.replace('"', "\"\""))
}
async fn csv(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    s.tinyml.repo.run(&id).await?;
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let pool = s.storage.pool().clone();
    tokio::spawn(async move {
        let mut connection = match pool.acquire().await {
            Ok(c) => c,
            Err(_) => {
                let _ = sender
                    .send(Err(std::io::Error::other("CSV connection failed")))
                    .await;
                return;
            }
        };
        let postgres = connection.backend_name() == "PostgreSQL";
        let mut tx = match connection.begin().await {
            Ok(t) => t,
            Err(_) => {
                let _ = sender
                    .send(Err(std::io::Error::other("CSV snapshot failed")))
                    .await;
                return;
            }
        };
        if postgres
            && sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx)
                .await
                .is_err()
        {
            let _ = sender
                .send(Err(std::io::Error::other("CSV snapshot failed")))
                .await;
            return;
        }
        let header = "sample_id,device_timestamp,server_timestamp,actual,predicted,confidence,correct,inference_ms,pipeline_ms,chip_temperature_c,free_heap_bytes,other_json\r\n";
        if sender
            .send(Ok(Bytes::from_static(header.as_bytes())))
            .await
            .is_err()
        {
            return;
        }
        let mut cursor = String::new();
        let mut timestamp = String::new();
        loop {
            let rows=sqlx::query_as::<_,ResultRow>("SELECT * FROM tinyml_results WHERE run_key=$1 AND (server_timestamp>$2 OR (server_timestamp=$2 AND id>$3)) ORDER BY server_timestamp,id LIMIT 500").bind(&id).bind(&timestamp).bind(&cursor).fetch_all(&mut *tx).await;
            let rows = match rows {
                Ok(rows) => rows,
                Err(_) => {
                    let _ = sender
                        .send(Err(std::io::Error::other("CSV database read failed")))
                        .await;
                    return;
                }
            };
            if rows.is_empty() {
                break;
            }
            let mut output = String::new();
            for r in rows {
                cursor = r.id;
                timestamp = r.server_timestamp.clone();
                let number = |v: Option<f64>| v.map(|n| n.to_string()).unwrap_or_default();
                let cells = [
                    r.sample_id.unwrap_or_default(),
                    r.device_timestamp.unwrap_or_default(),
                    r.server_timestamp,
                    r.actual_class.unwrap_or_default(),
                    r.predicted_class.unwrap_or_default(),
                    number(r.confidence),
                    r.correct.map(|n| (n == 1).to_string()).unwrap_or_default(),
                    number(r.inference_us.map(|n| n / 1000.0)),
                    number(r.total_pipeline_us.map(|n| n / 1000.0)),
                    number(r.chip_temperature_c),
                    r.free_heap_bytes.map(|n| n.to_string()).unwrap_or_default(),
                    r.other_json,
                ];
                output.push_str(
                    &cells
                        .iter()
                        .map(|v| csv_cell(v))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                output.push_str("\r\n");
            }
            if sender.send(Ok(Bytes::from(output))).await.is_err() {
                return;
            }
        }
    });
    Ok((
        [
            ("content-type", "text/csv; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=\"tinyml-results.csv\"",
            ),
        ],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver)),
    )
        .into_response())
}

#[derive(Deserialize)]
struct Hello {
    token: String,
}
async fn stream(
    State(s): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !s.config.allowed_origins.iter().any(|o| o == origin) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "ORIGIN_DENIED",
            "Origin is not allowed",
        ));
    }
    Ok(ws.max_message_size(4096).on_upgrade(move|mut socket|async move{
        let Ok(Some(Ok(Message::Text(text))))=tokio::time::timeout(Duration::from_secs(5),socket.recv()).await else{return;};
        let Ok(hello)=serde_json::from_str::<Hello>(&text) else{return;};
        if !crate::console::authorized(&s.console,&hello.token){return;}
        let mut events=s.tinyml.events.subscribe();
        let _=socket.send(Message::Text(json!({"type":"tinyml.connected"}).to_string().into())).await;
        let mut ping=tokio::time::interval(Duration::from_secs(20));
        loop {tokio::select! {
            event=events.recv()=>{
                let value=match event {Ok(v)=>v,Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>json!({"type":"tinyml.resync"}),Err(_)=>break};
                if !matches!(tokio::time::timeout(Duration::from_secs(10),socket.send(Message::Text(value.to_string().into()))).await,Ok(Ok(()))){break;}
            },
            incoming=socket.recv()=>if matches!(incoming,None|Some(Err(_))|Some(Ok(Message::Close(_)))){break;},
            _=ping.tick()=>if socket.send(Message::Ping(Vec::new().into())).await.is_err(){break;}
        }}
    }))
}
