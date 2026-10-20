//! Stable ESP32-S3 runtime.
//!
//! The runtime owns platform initialization, Wi-Fi, OTA and rollback. The selected
//! `device-app` Cargo dependency supplies the product-specific application.

use std::time::Duration;
mod remote_console;

use anyhow::{Context, Result};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop, hal::peripherals::Peripherals, nvs::EspDefaultNvsPartition,
};
use firmware_app_api::{AppContext, ApplicationPeripherals, DeviceInfo};
use log::{error, info};
use ota_runtime::{
    mark_running_image_valid, reject_running_image_and_reboot, running_image_is_unverified,
    OtaConfig,
};
use wifi_manager::{WifiConfig, WifiManager};

const WIFI_SSID: &str = match option_env!("WIFI_SSID") {
    Some(value) => value,
    None => "CHANGE_ME",
};
const WIFI_PASS: &str = match option_env!("WIFI_PASS") {
    Some(value) => value,
    None => "CHANGE_ME",
};
const OTA_SERVER_URL: &str = match option_env!("OTA_SERVER_URL") {
    Some(value) => value,
    None => "CHANGE_ME",
};
const DEVICE_ID_OVERRIDE: Option<&str> = option_env!("DEVICE_ID");
const DEVICE_NAME: &str = match option_env!("DEVICE_NAME") {
    Some(value) => value,
    None => "ESP32-S3 Managed Device",
};
const FIRMWARE_VERSION: &str = match option_env!("FIRMWARE_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

esp_idf_svc::sys::esp_app_desc!();

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();

    let peripherals = Peripherals::take().context("ESP32 peripherals already acquired")?;
    let (modem, application_peripherals) = ApplicationPeripherals::split_esp32s3(peripherals);
    let system_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let mut wifi = WifiManager::new(
        modem,
        system_loop,
        nvs,
        WifiConfig::bootstrap(WIFI_SSID, WIFI_PASS),
    )?;
    let station_mac = wifi.station_mac()?;
    let _sntp = esp_idf_svc::sntp::EspSntp::new_default()?;
    if remote_console::initialize().is_err() {
        // A logging/broker configuration error must not lock out local Wi-Fi setup.
        // Initialization can fail after installing the remote logger; in that case
        // retain it (it still writes to USB) rather than installing a second one.
        static SERIAL_FALLBACK: esp_idf_svc::log::EspIdfLogger<()> =
            esp_idf_svc::log::EspIdfLogger::new(());
        if log::set_logger(&SERIAL_FALLBACK).is_ok() {
            log::set_max_level(log::LevelFilter::Info);
        }
        println!("Remote console initialization failed. Check MQTT configuration; local Wi-Fi setup remains available.");
    }
    wifi.start()?;
    let device_id = DEVICE_ID_OVERRIDE
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format_device_id(station_mac));
    let network = wifi.network_handle();
    let device = DeviceInfo {
        device_id: device_id.clone(),
        name: DEVICE_NAME.to_owned(),
        chip: "esp32s3".to_owned(),
        firmware_version: FIRMWARE_VERSION.to_owned(),
    };
    let mut context = AppContext {
        device,
        network,
        peripherals: application_peripherals,
    };

    let unverified_image = running_image_is_unverified()?;
    let application_state = match device_app::setup(&mut context) {
        Ok(application_state) => application_state,
        Err(failure) => return fail_startup(unverified_image, failure),
    };
    if unverified_image {
        mark_running_image_valid()?;
        info!("Application setup passed; new OTA image marked valid");
    }

    let ota_config = OtaConfig {
        server_url: OTA_SERVER_URL.trim_end_matches('/').to_owned(),
        device_token: option_env!("DEVICE_OTA_TOKEN").unwrap_or("").to_owned(),
        device_id,
        device_name: DEVICE_NAME.to_owned(),
        chip: "esp32s3".to_owned(),
        current_version: FIRMWARE_VERSION.to_owned(),
        poll_interval: Duration::from_secs(30),
        reconnect_interval: Duration::from_secs(10),
    };
    std::thread::Builder::new()
        .name("ota-supervisor".to_owned())
        .stack_size(16 * 1024)
        .spawn(move || ota_runtime::run(wifi, ota_config, unverified_image))
        .context("could not start OTA supervisor task")?;

    info!("Starting managed device application main function");
    device_app::main(context, application_state)
}

fn format_device_id(mac: [u8; 6]) -> String {
    format!(
        "ESP32-S3-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

fn fail_startup(unverified_image: bool, failure: anyhow::Error) -> Result<()> {
    error!("Managed application startup failed: {failure:#}");
    if unverified_image {
        error!("Rejecting unverified image and requesting rollback");
        return Err(reject_running_image_and_reboot());
    }
    Err(failure)
}
