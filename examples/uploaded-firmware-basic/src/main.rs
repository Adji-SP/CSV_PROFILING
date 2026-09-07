//! Minimal function-based firmware intended to be uploaded as a ZIP.

use std::{thread, time::Duration};

use anyhow::Result;
use firmware_app_api::AppContext;
use log::info;

pub fn setup(context: &mut AppContext) -> Result<u64> {
    // Take and configure required GPIO, I2C, SPI, or UART resources here. Returning
    // an error during setup rejects a newly installed image when rollback is enabled.
    info!(
        "Firmware setup on {} running v{}",
        context.device.device_id, context.device.firmware_version
    );
    Ok(0)
}

pub fn main(context: AppContext, mut cycles: u64) -> Result<()> {
    loop {
        cycles += 1;
        let network = context.network.snapshot();
        info!(
            "Firmware cycle {}; connected={}, ip={:?}",
            cycles,
            context.network.is_connected(),
            network.ip_address
        );
        thread::sleep(Duration::from_secs(10));
    }
}
