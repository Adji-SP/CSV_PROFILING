//! Default application compiled into the initial USB-flashed runtime.

use std::{thread, time::Duration};

use anyhow::Result;
use firmware_app_api::AppContext;
use log::info;

pub fn setup(context: &mut AppContext) -> Result<()> {
    info!(
        "Default application initialized for {}",
        context.device.device_id
    );
    Ok(())
}

pub fn main(context: AppContext, _state: ()) -> Result<()> {
    loop {
        let network = context.network.snapshot();
        info!(
            "Default application alive; network={:?}, ip={:?}",
            network.state, network.ip_address
        );
        thread::sleep(Duration::from_secs(30));
    }
}
