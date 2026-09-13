//! Pull-based OTA agent used by the stable ESP32 runtime.

use std::{thread, time::Duration};

use anyhow::{bail, ensure, Context, Result};
use embedded_svc::http::client::Client as HttpClient;
use esp_idf_svc::{
    http::client::{EspHttpConnection, Method},
    io::{utils::try_read_full, Write},
    ota::{EspOta, SlotState},
};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wifi_manager::WifiManager;

#[derive(Debug, Clone)]
pub struct OtaConfig {
    pub server_url: String,
    pub device_id: String,
    pub device_name: String,
    pub chip: String,
    pub current_version: String,
    pub poll_interval: Duration,
    pub reconnect_interval: Duration,
}

#[derive(Serialize)]
struct RegisterRequest<'a> {
    device_id: &'a str,
    name: &'a str,
    chip: &'a str,
    current_version: &'a str,
    ip_address: &'a str,
}

#[derive(Serialize)]
struct HeartbeatRequest<'a> {
    current_version: &'a str,
    status: &'a str,
    ip_address: &'a str,
}

#[derive(Serialize)]
struct OtaStatusRequest<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct UpdateResponse {
    update_available: bool,
    firmware_id: Option<String>,
    version: Option<String>,
    target: Option<String>,
    size: Option<u64>,
    sha256: Option<String>,
    download_url: Option<String>,
}

pub fn run(mut wifi: WifiManager, config: OtaConfig, report_success_after_boot: bool) -> ! {
    if let Err(error) = validate_config(&config) {
        error!("OTA configuration is invalid: {error:#}");
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    let mut registered_ip: Option<String> = None;
    let mut pending_boot_report = report_success_after_boot;

    loop {
        let ip_address = match wifi.ensure_connected() {
            Ok(ip_address) => ip_address,
            Err(_) => {
                registered_ip = None;
                thread::sleep(config.reconnect_interval);
                continue;
            }
        };

        match run_cycle(
            &config,
            &ip_address,
            registered_ip.as_deref() != Some(ip_address.as_str()),
            pending_boot_report,
        ) {
            Ok(cycle) => {
                registered_ip = Some(ip_address);
                if cycle.boot_report_sent {
                    pending_boot_report = false;
                }
            }
            Err(failure) => {
                error!("OTA cycle failed: {failure:#}");
                registered_ip = None;
            }
        }
        thread::sleep(config.poll_interval);
    }
}

struct CycleResult {
    boot_report_sent: bool,
}

fn run_cycle(
    config: &OtaConfig,
    ip_address: &str,
    register_required: bool,
    report_success_after_boot: bool,
) -> Result<CycleResult> {
    let mut api_client = new_http_client()?;
    let mut download_client = new_http_client()?;
    if register_required {
        register(&mut api_client, config, ip_address)?;
    }
    heartbeat(&mut api_client, config, ip_address)?;

    let mut boot_report_sent = false;
    if report_success_after_boot {
        report_ota_status(
            &mut api_client,
            config,
            "success",
            Some(100),
            Some(&config.current_version),
            None,
        )?;
        boot_report_sent = true;
    }

    match get_update(&mut api_client, config)? {
        Some(update) => {
            if let Err(failure) = apply_update(
                &mut download_client,
                &mut api_client,
                config,
                &update,
            ) {
                error!("OTA installation failed: {failure:#}");
                let failure_text = format!("{failure:#}");
                let _ = report_ota_status(
                    &mut api_client,
                    config,
                    "failed",
                    None,
                    None,
                    Some(&failure_text),
                );
            }
        }
        None => info!("No OTA update available"),
    }
    Ok(CycleResult { boot_report_sent })
}

fn validate_config(config: &OtaConfig) -> Result<()> {
    if config.server_url.contains("localhost") || config.server_url.contains("127.0.0.1") {
        bail!("OTA server URL must use the dashboard PC's LAN address, not localhost");
    }
    if !(config.server_url.starts_with("http://") || config.server_url.starts_with("https://")) {
        bail!("OTA server URL must begin with http:// or https://");
    }
    Ok(())
}

fn new_http_client() -> Result<HttpClient<EspHttpConnection>> {
    Ok(HttpClient::wrap(EspHttpConnection::new(
        &Default::default(),
    )?))
}

fn register(
    client: &mut HttpClient<EspHttpConnection>,
    config: &OtaConfig,
    ip_address: &str,
) -> Result<()> {
    post_json(
        client,
        &format!("{}/api/ota/devices/register", config.server_url),
        &RegisterRequest {
            device_id: &config.device_id,
            name: &config.device_name,
            chip: &config.chip,
            current_version: &config.current_version,
            ip_address,
        },
    )
}

fn heartbeat(
    client: &mut HttpClient<EspHttpConnection>,
    config: &OtaConfig,
    ip_address: &str,
) -> Result<()> {
    post_json(
        client,
        &format!(
            "{}/api/ota/devices/{}/heartbeat",
            config.server_url, config.device_id
        ),
        &HeartbeatRequest {
            current_version: &config.current_version,
            status: "online",
            ip_address,
        },
    )
}

fn get_update(
    client: &mut HttpClient<EspHttpConnection>,
    config: &OtaConfig,
) -> Result<Option<UpdateResponse>> {
    let url = format!(
        "{}/api/ota/devices/{}/update",
        config.server_url, config.device_id
    );
    let request = client.request(Method::Get, &url, &[("accept", "application/json")])?;
    let mut response = request.submit()?;
    if response.status() != 200 {
        bail!("Update endpoint returned HTTP {}", response.status());
    }
    let mut buffer = [0_u8; 4096];
    let count = try_read_full(&mut response, &mut buffer).map_err(|error| error.0)?;
    let update: UpdateResponse = serde_json::from_slice(&buffer[..count])?;
    if update.version.as_deref() == Some(config.current_version.as_str()) {
        return Ok(None);
    }
    Ok(update.update_available.then_some(update))
}

fn apply_update(
    download_client: &mut HttpClient<EspHttpConnection>,
    status_client: &mut HttpClient<EspHttpConnection>,
    config: &OtaConfig,
    update: &UpdateResponse,
) -> Result<()> {
    let firmware_id = update
        .firmware_id
        .as_deref()
        .context("missing firmware_id")?;
    let target_version = update.version.as_deref().context("missing version")?;
    let target = update.target.as_deref().context("missing firmware target")?;
    ensure!(
        target == config.chip,
        "Firmware target {target} does not match device chip {}",
        config.chip
    );
    let expected_size = update.size.context("missing firmware size")?;
    ensure!(expected_size > 0, "Firmware image is empty");
    let expected_size_usize = usize::try_from(expected_size)
        .context("Firmware image is too large for this device")?;
    let expected_sha256 = update.sha256.as_deref().context("missing SHA-256")?;
    ensure!(
        expected_sha256.len() == 64
            && expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Firmware SHA-256 metadata is invalid"
    );
    let download_url = update
        .download_url
        .as_deref()
        .context("missing download URL")?;
    info!("Downloading firmware {firmware_id} v{target_version} ({expected_size} bytes)");

    let request = download_client.request(
        Method::Get,
        download_url,
        &[("accept", "application/octet-stream")],
    )?;
    let mut response = request.submit()?;
    if response.status() != 200 {
        bail!("Firmware download returned HTTP {}", response.status());
    }

    let mut ota = EspOta::new().context("cannot obtain ESP-IDF OTA service")?;
    let mut writer = ota
        .initiate_update_with_known_size(expected_size_usize)
        .context("cannot open inactive OTA partition")?;
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut last_reported = 0_u8;
    let mut chunk = [0_u8; 4096];

    report_ota_status(status_client, config, "downloading", Some(0), None, None)?;
    loop {
        let count = response.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        writer.write(&chunk[..count])?;
        hasher.update(&chunk[..count]);
        received += count as u64;
        let progress = ((received.saturating_mul(100) / expected_size.max(1)).min(100)) as u8;
        if progress >= last_reported.saturating_add(5) {
            report_ota_status(
                status_client,
                config,
                "downloading",
                Some(progress),
                None,
                None,
            )?;
            last_reported = progress;
        }
    }

    if received != expected_size {
        writer.abort()?;
        bail!("Firmware size mismatch: expected {expected_size}, received {received}");
    }
    report_ota_status(status_client, config, "verifying", Some(100), None, None)?;
    let actual_sha256 = format!("{:x}", hasher.finalize());
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        writer.abort()?;
        bail!("SHA256 mismatch");
    }

    report_ota_status(status_client, config, "installing", Some(100), None, None)?;
    writer
        .complete()
        .context("ESP-IDF rejected the OTA image")?;
    report_ota_status(status_client, config, "rebooting", Some(100), None, None)?;
    thread::sleep(Duration::from_millis(250));
    esp_idf_svc::hal::reset::restart();
}

fn report_ota_status(
    client: &mut HttpClient<EspHttpConnection>,
    config: &OtaConfig,
    status: &str,
    progress: Option<u8>,
    current_version: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    post_json(
        client,
        &format!(
            "{}/api/ota/devices/{}/ota-status",
            config.server_url, config.device_id
        ),
        &OtaStatusRequest {
            status,
            progress,
            current_version,
            error,
        },
    )
}

fn post_json<T: Serialize>(
    client: &mut HttpClient<EspHttpConnection>,
    url: &str,
    body: &T,
) -> Result<()> {
    let payload = serde_json::to_vec(body)?;
    let content_length = payload.len().to_string();
    let headers = [
        ("content-type", "application/json"),
        ("content-length", content_length.as_str()),
    ];
    let mut request = client.post(url, &headers)?;
    request.write_all(&payload)?;
    request.flush()?;
    let mut response = request.submit()?;
    let status = response.status();
    let mut discard = [0_u8; 256];
    while response.read(&mut discard)? != 0 {}
    if !(200..300).contains(&status) {
        bail!("POST {url} returned HTTP {status}");
    }
    Ok(())
}

pub fn running_image_is_unverified() -> Result<bool> {
    let ota = EspOta::new()?;
    let slot = ota.get_running_slot()?;
    if slot.state == SlotState::Unknown {
        warn!("Running OTA slot state is unknown; leaving it unchanged");
    }
    Ok(slot.state == SlotState::Unverified)
}

pub fn mark_running_image_valid() -> Result<()> {
    let mut ota = EspOta::new()?;
    ota.mark_running_slot_valid()
        .context("could not mark the running OTA image valid")
}

pub fn reject_running_image_and_reboot() -> anyhow::Error {
    match EspOta::new() {
        Ok(mut ota) => ota.mark_running_slot_invalid_and_reboot().into(),
        Err(error) => error.into(),
    }
}
