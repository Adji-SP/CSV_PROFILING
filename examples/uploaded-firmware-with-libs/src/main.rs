//! Function-based managed firmware demonstrating ordinary Cargo libraries.

use std::{thread, time::Duration};

use anyhow::{ensure, Result};
use firmware_app_api::AppContext;
use heapless::Deque;
use log::info;
use serde::Serialize;

#[derive(Serialize)]
struct Telemetry<'a> {
    device_id: &'a str,
    sample: u16,
    connected: bool,
}

pub fn setup(context: &mut AppContext) -> Result<Deque<u16, 16>> {
    ensure!(
        context.device.chip == "esp32s3",
        "this sample supports ESP32-S3 only"
    );
    Ok(Deque::new())
}

pub fn main(context: AppContext, mut samples: Deque<u16, 16>) -> Result<()> {
    let mut next_sample = 0_u16;
    loop {
        if samples.is_full() {
            samples.pop_front();
        }
        samples.push_back(next_sample).ok();
        let message = Telemetry {
            device_id: &context.device.device_id,
            sample: next_sample,
            connected: context.network.is_connected(),
        };
        info!("Telemetry: {}", serde_json::to_string(&message)?);
        next_sample = next_sample.wrapping_add(1);
        thread::sleep(Duration::from_secs(10));
    }
}
