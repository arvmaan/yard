use std::{ffi::OsString, ops::RangeInclusive, time::Duration};

pub const MIN_HERDR_PROTOCOL: u32 = 19;
pub const MAX_HERDR_PROTOCOL: u32 = 22;

#[derive(Debug, Clone)]
pub struct HerdrConfig {
    pub binary: OsString,
    pub supported_protocols: RangeInclusive<u32>,
    pub request_timeout: Duration,
    /// Timeout for `pane.read` history reads. For an idle full-screen agent
    /// Herdr pages the alternate-screen transcript for up to 15 s, then
    /// restores the view for up to 5 more seconds before it answers (with the
    /// result or its truncated fallback), so this must exceed that 20 s worst
    /// case rather than `request_timeout`. It stays below the web's 30 s
    /// `TERMINAL_OUTPUT_REQUEST_TIMEOUT_MS`.
    pub pane_read_timeout: Duration,
    pub initial_prompt_blocked_retry_timeout: Duration,
    pub max_discovery_bytes: usize,
    pub max_response_bytes: usize,
}

impl Default for HerdrConfig {
    fn default() -> Self {
        Self {
            binary: OsString::from("herdr"),
            supported_protocols: MIN_HERDR_PROTOCOL..=MAX_HERDR_PROTOCOL,
            request_timeout: Duration::from_secs(5),
            pane_read_timeout: Duration::from_secs(25),
            initial_prompt_blocked_retry_timeout: Duration::from_secs(3),
            max_discovery_bytes: 1024 * 1024,
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}
