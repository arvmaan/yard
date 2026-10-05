//! Retains a read-only terminal transcript after an assignment ends.
//!
//! Completion revokes live terminal access, and ending the session queues the
//! runtime for cleanup, so the pane's recent output is read once and stored.
//! Every read is identity-guarded: a fresh inventory is taken immediately
//! before and after the read, and the captured runtime must still be exactly
//! the pane the assignment ran in. A capture whose runtime is gone or reused
//! expires; a Herdr outage only backs off, so no transcript is lost to it.

use std::{sync::Arc, time::Duration};

use tracing::{debug, warn};
use yard_domain::RuntimeInventory;
use yard_store::{
    CapturedTranscriptText, PendingTranscriptCapture, ProjectStoreError, TranscriptCaptureExpiry,
    TranscriptCaptureOutcome, YardStore,
};

use crate::allocation_service::RuntimeRetirementRequest;
use crate::intervention_service::{
    MAX_TERMINAL_OUTPUT_LINES, RuntimeIntervention, RuntimeOutputRequest,
};
use crate::inventory_service::{InventorySource, RetirementResolution, retirement_resolution};

pub const TRANSCRIPT_CAPTURE_INTERVAL: Duration = Duration::from_secs(1);
const TRANSCRIPT_CAPTURE_BATCH_SIZE: usize = 20;
const TRANSCRIPT_CAPTURE_CLAIM_TTL_MS: u64 = 30_000;
const TRANSCRIPT_RETRY_BASE_MS: u64 = 5_000;
const TRANSCRIPT_RETRY_CAP_MS: u64 = 10 * 60_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscriptCaptureReport {
    pub attempted: usize,
    pub stored: usize,
    pub expired: usize,
    pub retrying: usize,
}

#[derive(Clone)]
pub struct TranscriptCaptureService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeIntervention>,
    store: Arc<dyn YardStore>,
}

/// What the captured runtime identity looks like in one fresh inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturedRuntimeState {
    /// The exact captured pane is present.
    Live,
    /// Nothing with the captured identity is observed.
    Absent,
    /// The identity is observed but no longer matches the captured pane.
    Changed,
}

enum CaptureStep {
    Stored,
    Expired(TranscriptCaptureExpiry),
    /// The store's commit-time guard already recorded the expiry.
    ExpiredAtCommit,
    Retry(String),
}

impl TranscriptCaptureService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeIntervention>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        Self {
            source,
            runtime,
            store,
        }
    }

    /// Attempt the capture jobs queued by one committed command.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when jobs cannot be claimed or their
    /// result cannot be persisted.
    pub async fn process_command(
        &self,
        command_id: &str,
    ) -> Result<TranscriptCaptureReport, ProjectStoreError> {
        self.process(Some(command_id)).await
    }

    /// Attempt one bounded batch of all due capture jobs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when jobs cannot be claimed or their
    /// result cannot be persisted.
    pub async fn process_pending(&self) -> Result<TranscriptCaptureReport, ProjectStoreError> {
        self.process(None).await
    }

    /// Retry durable capture work forever. Each pass is independent, so a
    /// store or Herdr outage never ends the loop (a returning background task
    /// stops the server).
    pub async fn run(self) {
        loop {
            match self.process_pending().await {
                Ok(report) if report.attempted > 0 => {
                    debug!(
                        attempted = report.attempted,
                        stored = report.stored,
                        expired = report.expired,
                        retrying = report.retrying,
                        "Processed transcript capture jobs"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        error = %error,
                        "Transcript capture pass failed; durable jobs will be retried"
                    );
                }
            }
            tokio::time::sleep(TRANSCRIPT_CAPTURE_INTERVAL).await;
        }
    }

    async fn process(
        &self,
        command_id: Option<&str>,
    ) -> Result<TranscriptCaptureReport, ProjectStoreError> {
        let jobs = self
            .store
            .claim_pending_transcript_captures(
                command_id,
                TRANSCRIPT_CAPTURE_BATCH_SIZE,
                TRANSCRIPT_CAPTURE_CLAIM_TTL_MS,
            )
            .await?;
        let mut report = TranscriptCaptureReport {
            attempted: jobs.len(),
            ..TranscriptCaptureReport::default()
        };
        for job in jobs {
            match self.capture(&job).await? {
                CaptureStep::Stored => report.stored += 1,
                CaptureStep::ExpiredAtCommit => report.expired += 1,
                CaptureStep::Expired(expiry) => {
                    self.store
                        .expire_transcript_capture(&job.id, &job.claim_token, expiry)
                        .await?;
                    report.expired += 1;
                }
                CaptureStep::Retry(message) => {
                    let retry_after_ms = retry_after_ms(job.attempts);
                    self.store
                        .fail_transcript_capture(
                            &job.id,
                            &job.claim_token,
                            &message,
                            retry_after_ms,
                        )
                        .await?;
                    report.retrying += 1;
                    warn!(
                        job_id = %job.id,
                        worker_id = %job.worker_id,
                        attempt = job.attempts.saturating_add(1),
                        retry_after_ms,
                        error = %message,
                        "Transcript capture could not read Herdr; it will retry"
                    );
                }
            }
        }
        Ok(report)
    }

    async fn capture(
        &self,
        job: &PendingTranscriptCapture,
    ) -> Result<CaptureStep, ProjectStoreError> {
        let guard = self
            .store
            .transcript_capture_guard(&job.id, &job.claim_token)
            .await?;
        if guard.worker_reallocated {
            return Ok(CaptureStep::Expired(
                TranscriptCaptureExpiry::WorkerReallocated,
            ));
        }
        if guard.bound_elsewhere {
            return Ok(CaptureStep::Expired(TranscriptCaptureExpiry::RuntimeReused));
        }
        match self.observe(job, guard.bound_to_worker).await {
            Ok(CapturedRuntimeState::Live) => {}
            Ok(state) => return Ok(CaptureStep::Expired(expiry_for(state))),
            Err(message) => return Ok(CaptureStep::Retry(message)),
        }
        let output = match self
            .runtime
            .read_output(RuntimeOutputRequest {
                request_id: uuid::Uuid::now_v7().to_string(),
                session: job.session.clone(),
                pane_id: job.pane_id.clone(),
                lines: MAX_TERMINAL_OUTPUT_LINES,
                format: yard_domain::TerminalOutputFormat::Text,
            })
            .await
        {
            Ok(output) => output,
            Err(error) => return Ok(CaptureStep::Retry(error.to_string())),
        };
        if output.pane_id != job.pane_id
            || output.workspace_id != job.workspace_id
            || output.tab_id != job.tab_id.as_deref().unwrap_or_default()
        {
            return Ok(CaptureStep::Expired(TranscriptCaptureExpiry::RuntimeReused));
        }
        match self.observe(job, guard.bound_to_worker).await {
            Ok(CapturedRuntimeState::Live) => {}
            Ok(state) => return Ok(CaptureStep::Expired(expiry_for(state))),
            Err(message) => return Ok(CaptureStep::Retry(message)),
        }
        let outcome = self
            .store
            .succeed_transcript_capture(
                &job.id,
                &job.claim_token,
                CapturedTranscriptText {
                    source: output.source,
                    format: output.format,
                    text: output.text,
                    truncated: output.truncated,
                },
            )
            .await?;
        Ok(match outcome {
            TranscriptCaptureOutcome::Stored => CaptureStep::Stored,
            TranscriptCaptureOutcome::Expired(_) => CaptureStep::ExpiredAtCommit,
        })
    }

    async fn observe(
        &self,
        job: &PendingTranscriptCapture,
        bound_to_worker: bool,
    ) -> Result<CapturedRuntimeState, String> {
        let inventory = self
            .source
            .inventory(&job.session)
            .await
            .map_err(|error| error.to_string())?;
        Ok(captured_runtime_state(&inventory, job, bound_to_worker))
    }
}

const fn expiry_for(state: CapturedRuntimeState) -> TranscriptCaptureExpiry {
    match state {
        CapturedRuntimeState::Absent => TranscriptCaptureExpiry::RuntimeClosed,
        CapturedRuntimeState::Live | CapturedRuntimeState::Changed => {
            TranscriptCaptureExpiry::RuntimeReused
        }
    }
}

fn retry_after_ms(attempts: u32) -> u64 {
    let multiplier = 1_u64.checked_shl(attempts).unwrap_or(u64::MAX);
    TRANSCRIPT_RETRY_BASE_MS
        .saturating_mul(multiplier)
        .min(TRANSCRIPT_RETRY_CAP_MS)
}

/// Classify the captured identity in one inventory.
///
/// A still-bound runtime uses the bound-runtime guard: the exact terminal,
/// pane, tab, workspace, and provider session. A detached runtime must also
/// resolve to `Target` under the retirement identity rules, which catch a
/// provider session that moved or a terminal ID that was reused.
fn captured_runtime_state(
    inventory: &RuntimeInventory,
    job: &PendingTranscriptCapture,
    bound_to_worker: bool,
) -> CapturedRuntimeState {
    if inventory.adapter != job.adapter || inventory.session != job.session {
        return CapturedRuntimeState::Changed;
    }
    if !bound_to_worker {
        match retirement_resolution(inventory, &retirement_request(job)) {
            RetirementResolution::Target => {}
            RetirementResolution::Absent => return CapturedRuntimeState::Absent,
            RetirementResolution::Conflict => return CapturedRuntimeState::Changed,
        }
    }
    let tab_id = job.tab_id.as_deref().unwrap_or_default();
    if let Some(worker) = inventory
        .workers
        .iter()
        .find(|worker| worker.terminal_id == job.terminal_id)
    {
        return if worker.workspace_id == job.workspace_id
            && worker.pane_id == job.pane_id
            && worker.tab_id == tab_id
            && worker.provider_session == job.provider_session
        {
            CapturedRuntimeState::Live
        } else {
            CapturedRuntimeState::Changed
        };
    }
    if let Some(pane) = inventory
        .panes
        .iter()
        .find(|pane| pane.terminal_id == job.terminal_id)
    {
        return if pane.workspace_id == job.workspace_id
            && pane.runtime_id == job.pane_id
            && pane.tab_id == tab_id
            && pane.provider_session == job.provider_session
        {
            CapturedRuntimeState::Live
        } else {
            CapturedRuntimeState::Changed
        };
    }
    if bound_to_worker {
        CapturedRuntimeState::Absent
    } else {
        // Resolution found the identity elsewhere (for example a provider
        // session that moved terminals); the captured pane is not it.
        CapturedRuntimeState::Changed
    }
}

fn retirement_request(job: &PendingTranscriptCapture) -> RuntimeRetirementRequest {
    RuntimeRetirementRequest {
        cleanup_id: job.id.clone(),
        adapter: job.adapter.clone(),
        session: job.session.clone(),
        workspace_id: job.workspace_id.clone(),
        terminal_id: job.terminal_id.clone(),
        tab_id: job.tab_id.clone(),
        pane_id: job.pane_id.clone(),
        provider_session: job.provider_session.clone(),
        owns_tab: false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yard_domain::{
        FocusObservation, ObservedStatus, ObservedWorker, ProviderSessionRef, RuntimeInventory,
    };
    use yard_store::PendingTranscriptCapture;

    use super::{CapturedRuntimeState, captured_runtime_state, retry_after_ms};

    fn provider(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn job() -> PendingTranscriptCapture {
        PendingTranscriptCapture {
            id: "job-1".to_owned(),
            command_id: "command-1".to_owned(),
            worker_id: "worker-1".to_owned(),
            assignment_id: Some("assignment-1".to_owned()),
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            tab_id: Some("tab-1".to_owned()),
            pane_id: "pane-1".to_owned(),
            provider_session: Some(provider("session-1")),
            attempts: 0,
            claim_token: "claim-1".to_owned(),
        }
    }

    fn observed(terminal_id: &str, pane_id: &str, session: &str) -> ObservedWorker {
        ObservedWorker {
            runtime_id: terminal_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            workspace_id: "workspace-1".to_owned(),
            tab_id: "tab-1".to_owned(),
            pane_id: pane_id.to_owned(),
            pane_instance_id: None,
            name: None,
            provider: Some("codex".to_owned()),
            display_provider: None,
            status: ObservedStatus::Idle,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 1,
            cwd: None,
            foreground_cwd: None,
            tokens: BTreeMap::new(),
            provider_session: Some(provider(session)),
            revision: 1,
        }
    }

    fn inventory(workers: Vec<ObservedWorker>) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.8.2".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            workers,
            child_agents: Vec::new(),
        }
    }

    #[test]
    fn captures_only_the_exact_captured_pane() {
        let live = inventory(vec![observed("terminal-1", "pane-1", "session-1")]);
        for bound in [true, false] {
            assert_eq!(
                captured_runtime_state(&live, &job(), bound),
                CapturedRuntimeState::Live
            );
        }
        let gone = inventory(Vec::new());
        for bound in [true, false] {
            assert_eq!(
                captured_runtime_state(&gone, &job(), bound),
                CapturedRuntimeState::Absent
            );
        }
    }

    #[test]
    fn a_reused_or_moved_identity_is_never_read() {
        // Same terminal ID, different agent session: reused.
        let reused = inventory(vec![observed("terminal-1", "pane-1", "session-2")]);
        for bound in [true, false] {
            assert_eq!(
                captured_runtime_state(&reused, &job(), bound),
                CapturedRuntimeState::Changed
            );
        }
        // The captured session moved to another terminal and pane.
        let moved = inventory(vec![observed("terminal-9", "pane-9", "session-1")]);
        assert_eq!(
            captured_runtime_state(&moved, &job(), false),
            CapturedRuntimeState::Changed
        );
        // The pane ID now belongs to the same terminal in another position.
        let repositioned = inventory(vec![observed("terminal-1", "pane-2", "session-1")]);
        assert_eq!(
            captured_runtime_state(&repositioned, &job(), true),
            CapturedRuntimeState::Changed
        );
    }

    #[test]
    fn retries_back_off_from_five_seconds_to_ten_minutes() {
        assert_eq!(retry_after_ms(0), 5_000);
        assert_eq!(retry_after_ms(1), 10_000);
        assert_eq!(retry_after_ms(7), 600_000);
        assert_eq!(retry_after_ms(40), 600_000);
    }
}
