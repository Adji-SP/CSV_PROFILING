//! Validation and timing policy; testable without ESP hardware.
use serde::{Deserialize, Serialize};

pub const FALLBACK_SECONDS: u64 = 60;
pub const ATTEMPT_SECONDS: u64 = 25;
pub const RETRY_SECONDS: u64 = 15;
pub const CLOSE_SECONDS: u64 = 20;
pub const PORTAL_IP: &str = "192.168.4.1";

// No Debug: do not accidentally log passwords.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WifiConfig {
    pub ssid: String,
    pub password: String,
}

impl WifiConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.ssid.is_empty() || self.ssid.len() > 32 || self.ssid.contains('\0') {
            return Err("Wi-Fi name must be 1-32 UTF-8 bytes without null characters.");
        }
        let valid = self.password.is_empty()
            || (8..=63).contains(&self.password.len())
            || (self.password.len() == 64 && self.password.bytes().all(|b| b.is_ascii_hexdigit()));
        if !valid || self.password.contains('\0') {
            return Err(
                "Use an 8-63 byte password, a 64-digit hexadecimal key, or an open network.",
            );
        }
        Ok(())
    }

    pub fn bootstrap(ssid: &str, password: &str) -> Option<Self> {
        if matches!(ssid, "" | "CHANGE_ME" | "your-wifi-name") || password == "CHANGE_ME" {
            return None;
        }
        let value = Self {
            ssid: ssid.into(),
            password: password.into(),
        };
        value.validate().ok().map(|_| value)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredSettings {
    pub schema: u8,
    pub generation: u64,
    pub wifi: WifiConfig,
}

pub fn decode_record(value: &str) -> Option<StoredSettings> {
    let record: StoredSettings = serde_json::from_str(value).ok()?;
    if record.schema != 1 || record.wifi.validate().is_err() {
        return None;
    }
    Some(record)
}

pub fn should_open_portal(has_settings: bool, disconnected_seconds: u64) -> bool {
    !has_settings || disconnected_seconds >= FALLBACK_SECONDS
}

/// Select the newest complete record; a torn write must not hide the older slot.
pub fn select_record(values: [Option<&str>; 2]) -> Option<(usize, StoredSettings)> {
    values
        .into_iter()
        .enumerate()
        .filter_map(|(slot, value)| value.and_then(decode_record).map(|record| (slot, record)))
        .max_by_key(|(_, record)| record.generation)
}

pub fn valid_portal_origin(host: Option<&str>, origin: Option<&str>) -> bool {
    matches!(host, Some("192.168.4.1" | "192.168.4.1:80"))
        && matches!(
            origin,
            None | Some("http://192.168.4.1" | "http://192.168.4.1:80")
        )
}

/// A bounded DNS parser for captive discovery; AAAA gets an empty answer.
pub fn dns_reply(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 17
        || query.len() > 512
        || query[2] & 0xf8 != 0
        || query[4..6] != [0, 1]
        || query[6..12] != [0; 6]
    {
        return None;
    }
    let mut end = 12;
    loop {
        let size = *query.get(end)? as usize;
        end += 1;
        if size == 0 {
            break;
        }
        if size > 63 || end + size > query.len() || end + size > 267 {
            return None;
        }
        end += size;
    }
    let tail = query.get(end..end + 4)?;
    if tail[2..] != [0, 1] {
        return None;
    }
    let is_a = tail[..2] == [0, 1];
    let mut reply = query[..end + 4].to_vec();
    reply[2] = 0x80 | (query[2] & 1);
    reply[3] = 0x80;
    reply[6..8].copy_from_slice(&[0, u8::from(is_a)]);
    if is_a {
        reply.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 192, 168, 4, 1]);
    }
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(password: &str) -> WifiConfig {
        WifiConfig {
            ssid: "Home Wi-Fi".into(),
            password: password.into(),
        }
    }
    #[test]
    fn validates_bytes_and_preserves_spaces() {
        assert!(config(" pass123 ").validate().is_ok());
        assert!(config("").validate().is_ok());
        assert!(config("short").validate().is_err());
        assert!(config(&"a".repeat(64)).validate().is_ok());
        assert!(config(&"z".repeat(64)).validate().is_err());
        assert!(config("abcdefgh\0").validate().is_err());
        let mut value = config("12345678");
        value.ssid = "é".repeat(17);
        assert!(value.validate().is_err());
    }
    #[test]
    fn optional_bootstrap_and_fallback_delay() {
        assert!(WifiConfig::bootstrap("", "").is_none());
        assert!(WifiConfig::bootstrap("CHANGE_ME", "CHANGE_ME").is_none());
        assert!(WifiConfig::bootstrap("Open network", "").is_some());
        assert!(should_open_portal(false, 0));
        assert!(!should_open_portal(true, 59));
        assert!(should_open_portal(true, 60));
    }
    #[test]
    fn stored_records_validate_schema_and_credentials() {
        let record = StoredSettings {
            schema: 1,
            generation: 7,
            wifi: config("12345678"),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(decode_record(&json).unwrap().generation, 7);
        assert!(decode_record(&json.replace("\"schema\":1", "\"schema\":2")).is_none());
        assert!(decode_record("broken").is_none());
    }
    #[test]
    fn rejects_cross_origin_requests() {
        assert!(valid_portal_origin(
            Some(PORTAL_IP),
            Some("http://192.168.4.1")
        ));
        assert!(valid_portal_origin(Some(PORTAL_IP), None));
        assert!(!valid_portal_origin(Some("evil.example"), None));
        assert!(!valid_portal_origin(
            Some(PORTAL_IP),
            Some("https://evil.example")
        ));
        assert!(!valid_portal_origin(Some(PORTAL_IP), Some("null")));
    }
    #[test]
    fn incomplete_new_slot_preserves_the_previous_working_credentials() {
        let old = serde_json::to_string(&StoredSettings {
            schema: 1,
            generation: 4,
            wifi: config("old-pass"),
        })
        .unwrap();
        let new = serde_json::to_string(&StoredSettings {
            schema: 1,
            generation: 5,
            wifi: config("new-pass"),
        })
        .unwrap();
        let selected = select_record([Some(&old), Some(&new)]).unwrap();
        assert_eq!(selected.0, 1);
        assert_eq!(selected.1.wifi.password, "new-pass");
        for length in 0..new.len() {
            let selected = select_record([Some(&old), Some(&new[..length])]).unwrap();
            assert_eq!(selected.0, 0);
            assert_eq!(selected.1.wifi.password, "old-pass");
        }
        assert!(select_record([None, Some("corrupt")]).is_none());
    }
    #[test]
    fn bounded_captive_dns() {
        let query = [
            0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, b'a', 0, 0, 1, 0, 1,
        ];
        let reply = dns_reply(&query).unwrap();
        assert_eq!(&reply[..2], &[0x12, 0x34]);
        assert_eq!(&reply[reply.len() - 4..], &[192, 168, 4, 1]);
        for len in 0..query.len() {
            assert!(dns_reply(&query[..len]).is_none());
        }
        let mut compressed = query;
        compressed[12] = 0xc0;
        assert!(dns_reply(&compressed).is_none());
        let mut ipv6 = query;
        ipv6[16] = 28;
        assert_eq!(dns_reply(&ipv6).unwrap()[7], 0);
    }
}
