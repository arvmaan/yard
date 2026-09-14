use std::{ffi::OsString, ops::RangeInclusive, time::Duration};

pub const MIN_HERDR_PROTOCOL: u32 = 19;
pub const MAX_HERDR_PROTOCOL: u32 = 22;

#[derive(Debug, Clone)]
pub struct HerdrConfig {
    pub binary: OsString,
    pub supported_protocols: RangeInclusive<u32>,
    pub request_timeout: Duration,
    pub max_discovery_bytes: usize,
    pub max_response_bytes: usize,
}

impl Default for HerdrConfig {
    fn default() -> Self {
        Self {
            binary: OsString::from("herdr"),
            supported_protocols: MIN_HERDR_PROTOCOL..=MAX_HERDR_PROTOCOL,
            request_timeout: Duration::from_secs(5),
            max_discovery_bytes: 1024 * 1024,
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}
