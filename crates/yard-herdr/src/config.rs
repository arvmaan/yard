use std::{ffi::OsString, time::Duration};

pub const HERDR_PROTOCOL: u32 = 19;

#[derive(Debug, Clone)]
pub struct HerdrConfig {
    pub binary: OsString,
    pub expected_protocol: u32,
    pub request_timeout: Duration,
    pub max_discovery_bytes: usize,
    pub max_response_bytes: usize,
}

impl Default for HerdrConfig {
    fn default() -> Self {
        Self {
            binary: OsString::from("herdr"),
            expected_protocol: HERDR_PROTOCOL,
            request_timeout: Duration::from_secs(5),
            max_discovery_bytes: 1024 * 1024,
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}
