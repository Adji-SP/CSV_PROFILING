//! Loopback-only integration test; no real credentials, MQTT broker or device required.
use crate::{
    config::Config,
    console::Console,
    services::{BuildCoordinator, LocalFirmwareBuilder},
    state::AppState,
    storage::Storage,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

#[tokio::test]
async fn console_auth_history_live_and_isolation() {
    let temp = tempfile::tempdir().expect("test directory");
    let root = temp.path().to_path_buf();
    let config = Arc::new(Config {
        console_only: true,
        bind_address: "127.0.0.1".into(),
        port: 0,
        public_base_url: "http://127.0.0.1".into(),
        project_dir: root.clone(),
        build_dir: root.clone(),
        data_dir: root.clone(),
        runtime_template_dir: root.clone(),
        cargo_target_dir: root.clone(),
        esp_idf_tools_dir: None,
        python_path: None,
        max_upload_bytes: 1024,
        max_extracted_bytes: 1024,
        allowed_origins: vec!["http://localhost:5000".into()],
        rust_toolchain: None,
    });
    let storage = Storage::connect(&format!(
        "sqlite:{}?mode=rwc",
        root.join("test.db").display()
    ))
    .await
    .expect("database");
    storage
        .upsert_device("node", "test", "esp32s3", "1", "127.0.0.1")
        .await
        .expect("register");
    let builder = Arc::new(LocalFirmwareBuilder::new(
        root.clone(),
        root.clone(),
        root.clone(),
        None,
        None,
        config.public_base_url.clone(),
        None,
    ));
    let coordinator = Arc::new(BuildCoordinator::new(
        storage.clone(),
        builder,
        root.clone(),
        root,
    ));
    let console = Console::for_test("a".repeat(64));
    let state = AppState {
        config,
        storage,
        coordinator,
        console: console.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::routes::router(state))
            .await
            .expect("server")
    });
    let request = |origin: &str| {
        let mut request = format!("ws://{address}/api/ota/console/ws")
            .into_client_request()
            .expect("request");
        request
            .headers_mut()
            .insert("origin", origin.parse().expect("origin"));
        request
    };
    assert!(
        connect_async(request("https://untrusted.example"))
            .await
            .is_err()
    );
    let (mut denied, _) = connect_async(request("http://localhost:5000"))
        .await
        .expect("upgrade");
    denied
        .send(Message::Text(
            r#"{"token":"wrong","device_id":"node"}"#.into(),
        ))
        .await
        .expect("hello");
    let closed = tokio::time::timeout(std::time::Duration::from_secs(3), denied.next())
        .await
        .expect("auth timeout");
    assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
    console
        .accept(
            "devices/node/logs",
            br#"{"device_id":"node","level":"INFO","message":"history"}"#,
        )
        .await;
    let (mut socket, _) = connect_async(request("http://localhost:5000"))
        .await
        .expect("upgrade");
    socket
        .send(Message::Text(
            serde_json::json!({"token":"a".repeat(64),"device_id":"node"})
                .to_string()
                .into(),
        ))
        .await
        .expect("hello");
    async fn text(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(Ok(Message::Text(s))) = socket.next().await {
                    return s.to_string();
                }
            }
        })
        .await
        .expect("text timeout")
    }
    assert!(text(&mut socket).await.contains("Authenticated"));
    assert!(text(&mut socket).await.contains("history"));
    console
        .accept(
            "devices/other/logs",
            br#"{"device_id":"other","level":"INFO","message":"private"}"#,
        )
        .await;
    console
        .accept(
            "devices/node/logs",
            br#"{"device_id":"node","level":"WARN","message":"live"}"#,
        )
        .await;
    assert!(text(&mut socket).await.contains("live"));
    // A hosted console must not mount the trusted build API.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut http = tokio::net::TcpStream::connect(address).await.expect("http");
    http.write_all(b"POST /api/ota/projects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.expect("write");
    let mut response = String::new();
    http.read_to_string(&mut response).await.expect("read");
    assert!(response.starts_with("HTTP/1.1 404"));
    server.abort();
}
