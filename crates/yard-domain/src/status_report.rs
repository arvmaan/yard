use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ORCHESTRATOR_STATUS_REPORT_VERSION: u64 = 1;
pub const MAX_STATUS_COMMAND_ID_BYTES: usize = 120;
pub const MAX_STATUS_TEXT_BYTES: usize = 4_096;
pub const MAX_STATUS_BLOCKER_BYTES: usize = 2_048;
pub const MAX_STATUS_BLOCKERS: usize = 32;
pub const MAX_STATUS_REPORT_LINE_BYTES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestratorStatusState {
    Working,
    NeedsAttention,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorStatusReport {
    pub version: u64,
    pub command_id: String,
    pub state: OrchestratorStatusState,
    pub last: String,
    pub next: String,
    pub blockers: Vec<String>,
}

impl OrchestratorStatusReport {
    /// Parse and validate one complete JSON line.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorStatusReportError`] when the line is oversized,
    /// is not exactly one object with the required shape, or contains an
    /// unsupported version, state, or bounded field.
    pub fn parse_line(line: &str) -> Result<Self, OrchestratorStatusReportError> {
        if line.contains(['\r', '\n']) {
            return Err(OrchestratorStatusReportError::MultipleLines);
        }
        if line.len() > MAX_STATUS_REPORT_LINE_BYTES {
            return Err(OrchestratorStatusReportError::LineTooLong {
                max_bytes: MAX_STATUS_REPORT_LINE_BYTES,
            });
        }
        let report = serde_json::from_str::<Self>(line)?;
        report.validate()
    }

    /// Scan terminal output from newest to oldest for the last valid
    /// standalone status-report line.
    #[must_use]
    pub fn scan_terminal_output(output: &str) -> Option<Self> {
        output
            .lines()
            .rev()
            .find_map(|line| Self::parse_line(line).ok())
    }

    /// Return the last valid standalone report only when it belongs to the
    /// authoritative expected command.
    #[must_use]
    pub fn scan_terminal_output_for_command(
        output: &str,
        expected_command_id: &str,
    ) -> Option<Self> {
        Self::scan_terminal_output(output).filter(|report| report.command_id == expected_command_id)
    }

    fn validate(self) -> Result<Self, OrchestratorStatusReportError> {
        if self.version != ORCHESTRATOR_STATUS_REPORT_VERSION {
            return Err(OrchestratorStatusReportError::UnsupportedVersion {
                version: self.version,
            });
        }
        validate_required("command_id", &self.command_id, MAX_STATUS_COMMAND_ID_BYTES)?;
        validate_bounded("last", &self.last, MAX_STATUS_TEXT_BYTES)?;
        validate_bounded("next", &self.next, MAX_STATUS_TEXT_BYTES)?;
        if self.blockers.len() > MAX_STATUS_BLOCKERS {
            return Err(OrchestratorStatusReportError::TooManyBlockers {
                max: MAX_STATUS_BLOCKERS,
            });
        }
        for blocker in &self.blockers {
            validate_required("blockers", blocker, MAX_STATUS_BLOCKER_BYTES)?;
        }
        Ok(self)
    }
}

#[derive(Debug, Error)]
pub enum OrchestratorStatusReportError {
    #[error("status report must occupy exactly one line")]
    MultipleLines,
    #[error("status report line exceeds {max_bytes} bytes")]
    LineTooLong { max_bytes: usize },
    #[error("status report is not a strict JSON object: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("unsupported status report version {version}")]
    UnsupportedVersion { version: u64 },
    #[error("{field} is required and may not have surrounding whitespace")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("status report has more than {max} blockers")]
    TooManyBlockers { max: usize },
}

fn validate_required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), OrchestratorStatusReportError> {
    if value.is_empty() || value.trim() != value {
        return Err(OrchestratorStatusReportError::Required { field });
    }
    validate_bounded(field, value, max_bytes)
}

fn validate_bounded(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), OrchestratorStatusReportError> {
    if value.len() > max_bytes {
        return Err(OrchestratorStatusReportError::TooLong { field, max_bytes });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_STATUS_BLOCKERS, MAX_STATUS_TEXT_BYTES, OrchestratorStatusReport,
        OrchestratorStatusReportError, OrchestratorStatusState,
    };

    #[test]
    fn scans_for_the_last_valid_standalone_object() {
        let output = concat!(
            "shell output\n",
            "{\"version\":1,\"command_id\":\"older\",\"state\":\"working\",",
            "\"last\":\"started\",\"next\":\"test\",\"blockers\":[]}\n",
            "{\"version\":2,\"command_id\":\"ignored\",\"state\":\"idle\",",
            "\"last\":\"\",\"next\":\"\",\"blockers\":[]}\n",
            "{\"version\":1,\"command_id\":\"latest\",\"state\":\"needs_attention\",",
            "\"last\":\"tests failed\",\"next\":\"inspect logs\",",
            "\"blockers\":[\"missing fixture\"]}\n",
            "trailing prose"
        );

        let report = OrchestratorStatusReport::scan_terminal_output(output).unwrap();

        assert_eq!(report.command_id, "latest");
        assert_eq!(report.state, OrchestratorStatusState::NeedsAttention);
        assert_eq!(report.blockers, ["missing fixture"]);
    }

    #[test]
    fn rejects_wrong_version_state_command_and_shape() {
        for line in [
            "{\"version\":2,\"command_id\":\"command\",\"state\":\"idle\",\"last\":\"\",\"next\":\"\",\"blockers\":[]}",
            "{\"version\":1,\"command_id\":\"command\",\"state\":\"completed\",\"last\":\"done\",\"next\":\"\",\"blockers\":[]}",
            "{\"version\":1,\"command_id\":\" \",\"state\":\"idle\",\"last\":\"\",\"next\":\"\",\"blockers\":[]}",
            "{\"version\":1,\"command_id\":\"command\",\"state\":\"idle\",\"last\":\"\",\"next\":\"\",\"blockers\":[],\"extra\":true}",
            "prefix {\"version\":1,\"command_id\":\"command\",\"state\":\"idle\",\"last\":\"\",\"next\":\"\",\"blockers\":[]}",
        ] {
            assert!(
                OrchestratorStatusReport::parse_line(line).is_err(),
                "unexpectedly accepted {line}"
            );
        }
    }

    #[test]
    fn rejects_a_newer_valid_object_for_the_wrong_command() {
        let output = concat!(
            "{\"version\":1,\"command_id\":\"expected\",\"state\":\"working\",",
            "\"last\":\"started\",\"next\":\"continue\",\"blockers\":[]}\n",
            "{\"version\":1,\"command_id\":\"forged\",\"state\":\"idle\",",
            "\"last\":\"claimed done\",\"next\":\"\",\"blockers\":[]}"
        );

        assert!(
            OrchestratorStatusReport::scan_terminal_output_for_command(output, "expected")
                .is_none()
        );
        assert_eq!(
            OrchestratorStatusReport::scan_terminal_output_for_command(output, "forged")
                .unwrap()
                .command_id,
            "forged"
        );
    }

    #[test]
    fn enforces_field_and_collection_bounds() {
        let oversized_text = "x".repeat(MAX_STATUS_TEXT_BYTES + 1);
        let line = serde_json::json!({
            "version": 1,
            "command_id": "command",
            "state": "working",
            "last": oversized_text,
            "next": "",
            "blockers": []
        })
        .to_string();
        assert!(matches!(
            OrchestratorStatusReport::parse_line(&line),
            Err(OrchestratorStatusReportError::TooLong { field: "last", .. })
        ));

        let line = serde_json::json!({
            "version": 1,
            "command_id": "command",
            "state": "idle",
            "last": "",
            "next": "",
            "blockers": vec!["blocked"; MAX_STATUS_BLOCKERS + 1]
        })
        .to_string();
        assert!(matches!(
            OrchestratorStatusReport::parse_line(&line),
            Err(OrchestratorStatusReportError::TooManyBlockers { .. })
        ));
    }
}
