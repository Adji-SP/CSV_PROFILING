//! Minimal function-based firmware intended to be uploaded as a ZIP.

use std::{ thread, time::Duration };

use anyhow::Result;
use firmware_app_api::AppContext;
use log::info;

fn delay(seconds: u64) {
    thread::sleep(Duration::from_secs(seconds));
}

pub fn setup(context: &mut AppContext) -> Result<u64> {
    info!(
        "Firmware setup on {} running v{}",
        context.device.device_id,
        context.device.firmware_version
    );
    Ok(0)
}

pub fn main(context: AppContext, mut cycles: u64) -> Result<()> {
    loop {
        cycles += 1;
        let network = context.network.snapshot();
        info!("ROARRRRRR!");
        info!(
            "Firmware cycle {}; connected={}, ip={:?}",
            cycles,
            context.network.is_connected(),
            network.ip_address
        );
        delay(1);
    }
}
