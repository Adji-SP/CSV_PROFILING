use crate::settings::{decode_record, select_record, StoredSettings, WifiConfig};
use anyhow::{Context, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};

pub struct SettingsStore {
    nvs: EspDefaultNvs,
    active_slot: usize,
    generation: u64,
}

impl SettingsStore {
    pub fn open(partition: EspDefaultNvsPartition) -> Result<(Self, Option<WifiConfig>)> {
        let nvs = EspDefaultNvs::new(partition, "wifi_setup", true)?;
        let mut first = [0u8; 1024];
        let mut second = [0u8; 1024];
        let newest = select_record([
            nvs.get_str("settings_a", &mut first).ok().flatten(),
            nvs.get_str("settings_b", &mut second).ok().flatten(),
        ]);
        let (active_slot, generation, wifi) = match newest {
            Some((slot, record)) => (slot, record.generation, Some(record.wifi)),
            None => (1, 0, None),
        };
        Ok((
            Self {
                nvs,
                active_slot,
                generation,
            },
            wifi,
        ))
    }

    pub fn save(&mut self, wifi: &WifiConfig) -> Result<()> {
        let generation = self
            .generation
            .checked_add(1)
            .context("Wi-Fi settings generation exhausted")?;
        let slot = 1 - self.active_slot;
        let key = ["settings_a", "settings_b"][slot];
        let record = StoredSettings {
            schema: 1,
            generation,
            wifi: wifi.clone(),
        };
        // Preserve the previous slot if power fails during the new write.
        self.nvs.set_str(key, &serde_json::to_string(&record)?)?;
        let mut buffer = [0u8; 1024];
        let readback = self
            .nvs
            .get_str(key, &mut buffer)?
            .and_then(decode_record)
            .context("Wi-Fi settings verification failed")?;
        anyhow::ensure!(
            readback.generation == generation && readback.wifi == *wifi,
            "Wi-Fi settings verification failed"
        );
        self.active_slot = slot;
        self.generation = generation;
        Ok(())
    }

    pub fn setup_password(&self) -> Result<String> {
        let mut buffer = [0u8; 80];
        if let Some(password) = self.nvs.get_str("ap_password", &mut buffer)? {
            if password.len() == 24 && password.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(password.to_owned());
            }
        }
        let password = random_token();
        self.nvs.set_str("ap_password", &password)?;
        Ok(password)
    }
}

pub fn random_token() -> String {
    // Called after Wi-Fi starts, when radio entropy is available.
    format!(
        "{:08x}{:08x}{:08x}",
        unsafe { esp_idf_svc::sys::esp_random() },
        unsafe { esp_idf_svc::sys::esp_random() },
        unsafe { esp_idf_svc::sys::esp_random() }
    )
}
