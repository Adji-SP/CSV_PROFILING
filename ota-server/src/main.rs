mod config;
mod errors;
mod models;
mod routes;
mod services;
mod state;
mod storage;

use std::sync::Arc;

use axum::{Router, extract::DefaultBodyLimit, http::HeaderValue};
use config::Config;
use errors::{ApiError, ApiResult};
use services::{BuildCoordinator, LocalFirmwareBuilder};
use state::AppState;
use storage::Storage;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ApiResult<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let config = Arc::new(Config::load()?);
    let storage = Storage::connect(&config.database_url()).await?;
    let builder = Arc::new(LocalFirmwareBuilder::new(
        config.build_dir.clone(),
        config.runtime_template_dir.clone(),
        config.public_base_url.clone(),
        config.rust_toolchain.clone(),
    ));
    let coordinator = Arc::new(BuildCoordinator::new(
        storage.clone(),
        builder,
        config.project_dir.clone(),
        config.build_dir.clone(),
    ));
    let state = AppState {
        config: Arc::clone(&config),
        storage,
        coordinator,
    };

    let cors = cors_layer(&config)?;
    let upload_limit = config.max_upload_bytes.saturating_add(1024 * 1024);
    let app = Router::new()
        .merge(routes::router(state))
        .layer(DefaultBodyLimit::max(upload_limit))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let address = config.socket_addr()?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            ApiError::internal(
                "SERVER_BIND_FAILED",
                format!("Could not bind OTA server to {address}: {error}"),
            )
        })?;

    println!("OTA Server running");
    println!("Local:\nhttp://localhost:{}", config.port);
    println!("\nLAN / public firmware URL:\n{}", config.public_base_url);
    if config.public_base_url.contains("127.0.0.1") || config.public_base_url.contains("localhost")
    {
        println!(
            "\nWarning: set OTA_PUBLIC_BASE_URL to this PC's LAN address before deploying to an ESP32."
        );
    }

    axum::serve(listener, app).await.map_err(|error| {
        ApiError::internal("SERVER_ERROR", "OTA server stopped unexpectedly")
            .with_details(error.to_string())
    })
}

fn cors_layer(config: &Config) -> ApiResult<CorsLayer> {
    let origins = config
        .allowed_origins
        .iter()
        .map(|origin| {
            origin.parse::<HeaderValue>().map_err(|error| {
                ApiError::internal(
                    "INVALID_CORS_ORIGIN",
                    format!("Invalid CORS origin '{origin}': {error}"),
                )
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(Any)
        .allow_headers(Any))
}
