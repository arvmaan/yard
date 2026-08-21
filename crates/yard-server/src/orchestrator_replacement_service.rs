use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tracing::warn;
use yard_domain::{
    ObservedWorker, Project, ReplaceProjectOrchestrator, ReplacedProjectOrchestrator,
    RuntimeInventory, RuntimeObservationState, RuntimeProcessState, WorkerRuntimeBinding,
};
use yard_store::{
    BeginProjectOrchestratorReplacement, OrchestratorReplacementPrepareIntent,
    OrchestratorReplacementRecoveryOutcome, OrchestratorReplacementRecoveryTarget,
    OrchestratorReplacementRuntimeCapture, OrchestratorReplacementRuntimeRole,
    OrchestratorReplacementStartEvidence, ProjectStoreError, YardStore,
};

use crate::{
    allocation_service::{
        RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, agent_name,
        assignment_prompt, provider_args, validate_supported_profile,
    },
    inventory_service::{InventoryServiceError, InventorySource},
    runtime_cleanup_service::RuntimeCleanupService,
    status_protocol::{with_orchestrator_status_contract, with_orchestrator_workflow},
};

#[derive(Clone)]
pub struct OrchestratorReplacementService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    store: Arc<dyn YardStore>,
    cleanup: RuntimeCleanupService,
    active_operations: Arc<ReplacementOperations>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrchestratorReplacementRecoveryReport {
    pub captures: usize,
    pub prepare_intents: usize,
    pub adopted_commands: usize,
    pub absent_captures: usize,
    pub conflicting_captures: usize,
    pub quarantined_captures: usize,
    pub deferred_items: usize,
}

const RECOVERY_POLL_INTERVAL: Duration = Duration::from_secs(30);
const RECOVERY_RETRY_BASE_MS: u64 = 60_000;
const RECOVERY_RETRY_CAP_MS: u64 = 3_600_000;
const STRANDED_PENDING_MESSAGE: &str = "Yard lost the final persistence acknowledgement for an orchestrator replacement; \
     captured runtimes require reconciliation";

#[derive(Default)]
struct ReplacementOperations {
    active: Mutex<HashSet<String>>,
    recovery_gate: AsyncMutex<()>,
}

fn replacement_operations(store: &Arc<dyn YardStore>) -> Arc<ReplacementOperations> {
    static OPERATIONS: OnceLock<Mutex<HashMap<usize, Weak<ReplacementOperations>>>> =
        OnceLock::new();
    let store_id = Arc::as_ptr(store).cast::<()>() as usize;
    let mut operations = OPERATIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    operations.retain(|_, operations| operations.strong_count() > 0);
    if let Some(existing) = operations.get(&store_id).and_then(Weak::upgrade) {
        return existing;
    }
    let created = Arc::new(ReplacementOperations::default());
    operations.insert(store_id, Arc::downgrade(&created));
    created
}

struct ActiveReplacement {
    command_id: String,
    operations: Arc<ReplacementOperations>,
}

impl Drop for ActiveReplacement {
    fn drop(&mut self) {
        self.operations
            .active
            .lock()
            .unwrap()
            .remove(&self.command_id);
    }
}

impl OrchestratorReplacementService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        let active_operations = replacement_operations(&store);
        Self {
            cleanup: RuntimeCleanupService::new(Arc::clone(&runtime), Arc::clone(&store)),
            source,
            runtime,
            store,
            active_operations,
        }
    }

    async fn begin_active_replacement(
        &self,
        command_id: &str,
    ) -> Result<ActiveReplacement, ProjectStoreError> {
        let _recovery = self.active_operations.recovery_gate.lock().await;
        let mut operations = self.active_operations.active.lock().unwrap();
        if !operations.insert(command_id.to_owned()) {
            return Err(ProjectStoreError::CommandInProgress);
        }
        Ok(ActiveReplacement {
            command_id: command_id.to_owned(),
            operations: Arc::clone(&self.active_operations),
        })
    }

    /// Reconcile every ambiguous prepared or started replacement capture once.
    ///
    /// Fresh inventory can prove absence, preserve a conflicting identity, or
    /// identify a present runtime that protocol 19 cannot safely retire. Only
    /// a confirmed started capture can be adopted as the current orchestrator.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorReplacementServiceError`] when inventory or
    /// durable recovery state cannot be loaded or persisted.
    pub async fn reconcile_ambiguous_replacements(
        &self,
    ) -> Result<OrchestratorReplacementRecoveryReport, OrchestratorReplacementServiceError> {
        self.reconcile_ambiguous_replacements_for(None).await
    }

    pub async fn run(self) {
        loop {
            tokio::time::sleep(RECOVERY_POLL_INTERVAL).await;
            match self.reconcile_ambiguous_replacements().await {
                Ok(report)
                    if report.captures > 0
                        || report.prepare_intents > 0
                        || report.deferred_items > 0 =>
                {
                    tracing::info!(
                        captures = report.captures,
                        prepare_intents = report.prepare_intents,
                        adopted_commands = report.adopted_commands,
                        absent_captures = report.absent_captures,
                        conflicting_captures = report.conflicting_captures,
                        quarantined_captures = report.quarantined_captures,
                        deferred_items = report.deferred_items,
                        "Reconciled due ambiguous orchestrator replacement recovery items"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        error = %error,
                        "Ambiguous orchestrator replacement background reconciliation failed; durable recovery items remain scheduled"
                    );
                }
            }
        }
    }

    async fn reconcile_ambiguous_replacement(
        &self,
        command_id: &str,
    ) -> Result<OrchestratorReplacementRecoveryReport, OrchestratorReplacementServiceError> {
        self.reconcile_ambiguous_replacements_for(Some(command_id))
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn reconcile_ambiguous_replacements_for(
        &self,
        command_id: Option<&str>,
    ) -> Result<OrchestratorReplacementRecoveryReport, OrchestratorReplacementServiceError> {
        if command_id.is_none() {
            let _recovery = self.active_operations.recovery_gate.lock().await;
            let active_command_ids = self
                .active_operations
                .active
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect();
            self.store
                .recover_stranded_pending_project_orchestrator_replacements(
                    active_command_ids,
                    STRANDED_PENDING_MESSAGE,
                )
                .await?;
        }
        let recoveries = self
            .store
            .list_ambiguous_orchestrator_replacement_recoveries(command_id)
            .await?;
        let mut inventories = HashMap::<(String, String), Result<RuntimeInventory, String>>::new();
        let mut report = OrchestratorReplacementRecoveryReport::default();
        for recovery in recoveries {
            let captures = recovery.captures;
            if let Some(intent) = recovery.prepare_intent {
                report.prepare_intents += 1;
                let inventory = replacement_recovery_inventory(
                    &self.source,
                    &mut inventories,
                    &intent.adapter,
                    &intent.session,
                )
                .await;
                let inventory = match inventory {
                    Ok(inventory) => inventory,
                    Err(message) => {
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            OrchestratorReplacementRecoveryTarget::PrepareIntent,
                            intent.recovery_attempts,
                            None,
                            &format!(
                                "Fresh inventory for replacement prepare intent failed: {message}"
                            ),
                            true,
                        )
                        .await?;
                        report.deferred_items += 1;
                        continue;
                    }
                };
                match resolve_prepare_intent(&inventory, &intent) {
                    PrepareIntentResolution::Prepared(runtime) => {
                        if let Err(error) = self
                            .store
                            .capture_ambiguous_orchestrator_replacement_prepared_runtime(
                                &recovery.command_id,
                                runtime.as_ref().clone(),
                            )
                            .await
                        {
                            if recovery_requires_operator_attention(&error) {
                                self.record_recovery_attempt(
                                    &recovery.command_id,
                                    OrchestratorReplacementRecoveryTarget::PrepareIntent,
                                    intent.recovery_attempts,
                                    Some(
                                        OrchestratorReplacementRecoveryOutcome::ConflictingReused,
                                    ),
                                    &format!(
                                        "Command-unique prepare intent matched exact topology, but durable capture was rejected: {error}"
                                    ),
                                    true,
                                )
                                .await?;
                                report.conflicting_captures += 1;
                                continue;
                            }
                            return Err(error.into());
                        }
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            OrchestratorReplacementRecoveryTarget::Runtime(
                                OrchestratorReplacementRuntimeRole::Prepared,
                            ),
                            0,
                            Some(
                                OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable,
                            ),
                            "Command-unique prepare intent matched exact tab, pane, and terminal topology; protocol 19 cannot retire the captured prepared runtime safely",
                            true,
                        )
                        .await?;
                        report.captures += 1;
                        report.quarantined_captures += 1;
                        continue;
                    }
                    PrepareIntentResolution::Deferred(detail) => {
                        let target = if intent.last_absence_observed_at_unix_ms.is_some() {
                            OrchestratorReplacementRecoveryTarget::PrepareIntentStaleObservation
                        } else {
                            OrchestratorReplacementRecoveryTarget::PrepareIntent
                        };
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            target,
                            intent.recovery_attempts,
                            None,
                            detail,
                            true,
                        )
                        .await?;
                        report.deferred_items += 1;
                        continue;
                    }
                    PrepareIntentResolution::Absent(detail) => {
                        let converged = intent.absence_observations > 0;
                        let persisted_detail = if converged {
                            detail.to_owned()
                        } else {
                            format!(
                                "{detail}; a second fresh absence after durable backoff is \
                                 required because the timed-out create may still complete"
                            )
                        };
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            OrchestratorReplacementRecoveryTarget::PrepareIntentAbsence {
                                observed_at_unix_ms: inventory.observed_at_unix_ms,
                            },
                            intent.recovery_attempts,
                            converged
                                .then_some(OrchestratorReplacementRecoveryOutcome::AbsentConverged),
                            &persisted_detail,
                            !converged,
                        )
                        .await?;
                        if converged {
                            report.absent_captures += 1;
                        } else {
                            report.deferred_items += 1;
                        }
                        continue;
                    }
                    resolution => {
                        let (outcome, detail) = resolution.persisted_outcome();
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            OrchestratorReplacementRecoveryTarget::PrepareIntent,
                            intent.recovery_attempts,
                            Some(outcome),
                            detail,
                            outcome_requires_retry(outcome),
                        )
                        .await?;
                        update_recovery_report(&mut report, outcome);
                        continue;
                    }
                }
            }

            let mut resolutions = Vec::with_capacity(captures.len());
            for capture in captures {
                if capture.recovery_outcome
                    == Some(OrchestratorReplacementRecoveryOutcome::AbsentConverged)
                {
                    continue;
                }
                let inventory = replacement_recovery_inventory(
                    &self.source,
                    &mut inventories,
                    &capture.runtime.adapter,
                    &capture.runtime.session,
                )
                .await;
                let inventory = match inventory {
                    Ok(inventory) => inventory,
                    Err(message) => {
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            OrchestratorReplacementRecoveryTarget::Runtime(capture.role),
                            capture.recovery_attempts,
                            None,
                            &format!(
                                "Fresh inventory for captured replacement runtime failed: {message}"
                            ),
                            true,
                        )
                        .await?;
                        report.deferred_items += 1;
                        continue;
                    }
                };
                match resolve_replacement_capture(&inventory, &capture) {
                    CaptureResolution::Deferred(detail) => {
                        self.record_recovery_attempt(
                            &recovery.command_id,
                            OrchestratorReplacementRecoveryTarget::Runtime(capture.role),
                            capture.recovery_attempts,
                            None,
                            detail,
                            true,
                        )
                        .await?;
                        report.deferred_items += 1;
                    }
                    resolution => resolutions.push((capture, resolution)),
                }
            }
            report.captures += resolutions.len();

            let adoptable = resolutions.iter().find_map(|(capture, resolution)| {
                if capture.role == OrchestratorReplacementRuntimeRole::Started
                    && capture.objective_delivery_confirmed
                {
                    if let CaptureResolution::Adoptable(runtime) = resolution {
                        return Some(runtime.as_ref().clone());
                    }
                }
                None
            });
            if let Some(runtime) = adoptable {
                match self
                    .store
                    .recover_project_orchestrator_replacement(&recovery.command_id, runtime)
                    .await
                {
                    Ok(mut replacement) => {
                        self.process_cleanup(&recovery.command_id, &mut replacement)
                            .await;
                        report.adopted_commands += 1;
                        continue;
                    }
                    Err(error) if recovery_requires_operator_attention(&error) => {
                        let detail = format!(
                            "Confirmed started runtime is present, but atomic project adoption was rejected: {error}"
                        );
                        for (capture, _) in &resolutions {
                            self.record_recovery_attempt(
                                &recovery.command_id,
                                OrchestratorReplacementRecoveryTarget::Runtime(capture.role),
                                capture.recovery_attempts,
                                Some(
                                    OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable,
                                ),
                                &detail,
                                true,
                            )
                            .await?;
                            report.quarantined_captures += 1;
                        }
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                }
            }

            for (capture, resolution) in resolutions {
                let (outcome, detail) = resolution.persisted_outcome();
                self.record_recovery_attempt(
                    &recovery.command_id,
                    OrchestratorReplacementRecoveryTarget::Runtime(capture.role),
                    capture.recovery_attempts,
                    Some(outcome),
                    detail,
                    outcome_requires_retry(outcome),
                )
                .await?;
                update_recovery_report(&mut report, outcome);
            }
        }
        Ok(report)
    }

    async fn record_recovery_attempt(
        &self,
        command_id: &str,
        target: OrchestratorReplacementRecoveryTarget,
        expected_attempts: u32,
        outcome: Option<OrchestratorReplacementRecoveryOutcome>,
        detail: &str,
        retry: bool,
    ) -> Result<(), ProjectStoreError> {
        let retry_after_ms = retry.then(|| {
            if matches!(
                target,
                OrchestratorReplacementRecoveryTarget::PrepareIntentAbsence { .. }
                    | OrchestratorReplacementRecoveryTarget::PrepareIntentStaleObservation
            ) {
                prepare_absence_confirmation_delay_ms()
            } else {
                recovery_retry_after_ms(expected_attempts)
            }
        });
        self.store
            .record_orchestrator_replacement_recovery_attempt(
                command_id,
                target,
                expected_attempts,
                outcome,
                detail,
                retry_after_ms,
            )
            .await
    }

    /// Replace one project orchestrator with a newly provisioned worker.
    ///
    /// The displaced runtime is durably captured before Herdr is called. The
    /// prepared replacement is claimed before start, both bindings are freshly
    /// verified, and ownership changes only in the final store transaction.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorReplacementServiceError`] for invalid or stale
    /// commands, unsupported profiles, unverifiable runtime identity, runtime
    /// failures, or persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn replace_project(
        &self,
        project_id: &str,
        command: ReplaceProjectOrchestrator,
    ) -> Result<ReplacedProjectOrchestrator, OrchestratorReplacementServiceError> {
        let command = command.normalize()?;
        let command_id = command.command_id.clone();
        let _active = self.begin_active_replacement(&command_id).await?;
        let context = match self
            .store
            .begin_project_orchestrator_replacement(project_id, command)
            .await?
        {
            BeginProjectOrchestratorReplacement::Replayed(mut replacement) => {
                self.process_cleanup(&command_id, &mut replacement).await;
                return Ok(*replacement);
            }
            BeginProjectOrchestratorReplacement::Started(context) => *context,
        };
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_project_orchestrator_replacement(&command_id, &message, false)
                .await?;
            return Err(OrchestratorReplacementServiceError::UnsupportedProfile(
                message,
            ));
        }
        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(error) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &error.to_string(), false)
                    .await?;
                return Err(OrchestratorReplacementServiceError::UnsupportedProfile(
                    error.to_string(),
                ));
            }
        };

        let cwd = match self
            .verify_target_and_displaced_runtime(
                &context.project,
                &context.command.expected_orchestrator_runtime,
            )
            .await
        {
            Ok(cwd) => cwd,
            Err(error) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &error.to_string(), false)
                    .await?;
                return Err(error);
            }
        };
        let objective = assignment_prompt(
            &context.command.objective,
            &context.command.role,
            &context.profile,
        );
        let objective = with_orchestrator_workflow(&objective, &context.workflow_profile)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let provision = RuntimeProvisionRequest {
            command_id: command_id.clone(),
            session: context.project.runtime.session.clone(),
            workspace_id: context.project.runtime.workspace_id.clone(),
            cwd,
            tab_label: context.prepare_tab_label,
            agent_name: agent_name(&command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt: with_orchestrator_status_contract(&objective, &command_id),
        };

        let prepared = match self
            .runtime
            .prepare_replacement_worker(provision.clone())
            .await
        {
            Ok(runtime) => runtime,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, false)
                    .await?;
                return Err(OrchestratorReplacementServiceError::RuntimeProvision(
                    message,
                ));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message, ambiguous, ..
            }) => {
                if ambiguous {
                    self.fail_ambiguous_replacement(&command_id, &message)
                        .await?;
                } else {
                    self.store
                        .fail_project_orchestrator_replacement(&command_id, &message, false)
                        .await?;
                }
                return Err(if ambiguous {
                    OrchestratorReplacementServiceError::RuntimeProvisionAmbiguous(message)
                } else {
                    OrchestratorReplacementServiceError::RuntimeProvision(message)
                });
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                let runtime = *runtime;
                let retained = self
                    .store
                    .claim_project_orchestrator_replacement_runtime(&command_id, runtime.clone())
                    .await;
                let started = match retained {
                    Ok(()) => {
                        self.store
                            .record_project_orchestrator_replacement_started_runtime(
                                &command_id,
                                runtime.clone(),
                                OrchestratorReplacementStartEvidence::Unverified,
                            )
                            .await
                    }
                    Err(error) => Err(error),
                };
                let message = match started {
                    Ok(()) => message,
                    Err(error) => {
                        let quarantine = self
                            .store
                            .quarantine_provisioning_runtime(&command_id, runtime)
                            .await;
                        format!(
                            "{message}; started runtime capture failed: {error}; \
                             quarantine result: {quarantine:?}"
                        )
                    }
                };
                self.fail_ambiguous_replacement(&command_id, &message)
                    .await?;
                return Err(OrchestratorReplacementServiceError::ObjectiveDeliveryFailed(message));
            }
        };
        if let Err(error) = self
            .store
            .claim_project_orchestrator_replacement_runtime(&command_id, prepared.clone())
            .await
        {
            self.fail_ambiguous_replacement(&command_id, &error.to_string())
                .await?;
            return Err(error.into());
        }

        let started = match self
            .runtime
            .start_prepared_replacement_worker(provision, prepared)
            .await
        {
            Ok(runtime) => runtime,
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                let runtime = *runtime;
                let message = match self
                    .store
                    .record_project_orchestrator_replacement_started_runtime(
                        &command_id,
                        runtime.clone(),
                        OrchestratorReplacementStartEvidence::Unverified,
                    )
                    .await
                {
                    Ok(()) => message,
                    Err(error) => {
                        let quarantine = self
                            .store
                            .quarantine_provisioning_runtime(&command_id, runtime)
                            .await;
                        format!(
                            "{message}; started runtime capture failed: {error}; \
                             quarantine result: {quarantine:?}"
                        )
                    }
                };
                self.fail_ambiguous_replacement(&command_id, &message)
                    .await?;
                return Err(OrchestratorReplacementServiceError::ObjectiveDeliveryFailed(message));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                mut message,
                mut ambiguous,
                started_runtime,
            }) => {
                if let Some(runtime) = started_runtime {
                    ambiguous = true;
                    let runtime = *runtime;
                    if let Err(error) = self
                        .store
                        .record_project_orchestrator_replacement_started_runtime(
                            &command_id,
                            runtime.clone(),
                            OrchestratorReplacementStartEvidence::Unverified,
                        )
                        .await
                    {
                        let quarantine = self
                            .store
                            .quarantine_provisioning_runtime(&command_id, runtime)
                            .await;
                        message = format!(
                            "{message}; unverified started runtime capture failed: {error}; \
                             quarantine result: {quarantine:?}"
                        );
                    }
                }
                if ambiguous {
                    self.fail_ambiguous_replacement(&command_id, &message)
                        .await?;
                } else {
                    self.store
                        .fail_project_orchestrator_replacement(&command_id, &message, false)
                        .await?;
                }
                return Err(if ambiguous {
                    OrchestratorReplacementServiceError::RuntimeProvisionAmbiguous(message)
                } else {
                    OrchestratorReplacementServiceError::RuntimeProvision(message)
                });
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.fail_ambiguous_replacement(&command_id, &message)
                    .await?;
                return Err(
                    OrchestratorReplacementServiceError::RuntimeProvisionAmbiguous(message),
                );
            }
        };
        if let Err(error) = self
            .store
            .record_project_orchestrator_replacement_started_runtime(
                &command_id,
                started.clone(),
                OrchestratorReplacementStartEvidence::Confirmed,
            )
            .await
        {
            let quarantine = self
                .store
                .quarantine_provisioning_runtime(&command_id, started)
                .await;
            let message = format!("{error}; quarantine result: {quarantine:?}");
            self.fail_ambiguous_replacement(&command_id, &message)
                .await?;
            return Err(error.into());
        }
        let replacement = match self.verify_replacement_runtime(started).await {
            Ok(runtime) => runtime,
            Err(error) => {
                self.fail_ambiguous_replacement(&command_id, &error.to_string())
                    .await?;
                return Err(error);
            }
        };
        if let Err(error) = self
            .verify_target_and_displaced_runtime(
                &context.project,
                &context.command.expected_orchestrator_runtime,
            )
            .await
        {
            self.fail_ambiguous_replacement(&command_id, &error.to_string())
                .await?;
            return Err(error);
        }

        let mut result = match self
            .store
            .finalize_project_orchestrator_replacement(&command_id, replacement)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                self.fail_ambiguous_replacement(&command_id, &error.to_string())
                    .await?;
                return Err(error.into());
            }
        };
        self.process_cleanup(&command_id, &mut result).await;
        Ok(result)
    }

    async fn verify_target_and_displaced_runtime(
        &self,
        project: &Project,
        expected: &WorkerRuntimeBinding,
    ) -> Result<String, OrchestratorReplacementServiceError> {
        if project.runtime.adapter != expected.adapter
            || project.runtime.session != expected.session
            || project.runtime.workspace_id != expected.workspace_id
        {
            return Err(OrchestratorReplacementServiceError::RuntimeIdentityChanged);
        }
        let expected_provider = expected
            .provider_session
            .as_ref()
            .ok_or(OrchestratorReplacementServiceError::RuntimeIdentityChanged)?;
        let inventory = self.source.inventory(&project.runtime.session).await?;
        if inventory.adapter != project.runtime.adapter
            || inventory.session != project.runtime.session
        {
            return Err(OrchestratorReplacementServiceError::RuntimeIdentityChanged);
        }
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == project.runtime.workspace_id)
            .ok_or_else(|| {
                OrchestratorReplacementServiceError::RuntimeWorkspaceMissing(
                    project.runtime.workspace_id.clone(),
                )
            })?;
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == expected.terminal_id)
            .ok_or(OrchestratorReplacementServiceError::RuntimeIdentityChanged)?;
        if observed.workspace_id != project.runtime.workspace_id
            || observed.pane_id != expected.pane_id
            || Some(observed.tab_id.as_str()) != expected.tab_id.as_deref()
            || observed.provider_session.as_ref() != Some(expected_provider)
            || !observed.interactive_ready
        {
            return Err(OrchestratorReplacementServiceError::RuntimeIdentityChanged);
        }
        workspace
            .worktree
            .as_ref()
            .map(|worktree| worktree.checkout_path.clone())
            .or_else(|| {
                observed
                    .foreground_cwd
                    .clone()
                    .or_else(|| observed.cwd.clone())
            })
            .ok_or(OrchestratorReplacementServiceError::RuntimeCwdUnavailable)
    }

    async fn verify_replacement_runtime(
        &self,
        mut runtime: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, OrchestratorReplacementServiceError> {
        let inventory = self.source.inventory(&runtime.session).await?;
        if inventory.adapter != runtime.adapter
            || inventory.session != runtime.session
            || !inventory
                .workspaces
                .iter()
                .any(|workspace| workspace.runtime_id == runtime.workspace_id)
        {
            return Err(OrchestratorReplacementServiceError::ReplacementUnverified);
        }
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == runtime.terminal_id)
            .ok_or(OrchestratorReplacementServiceError::ReplacementUnverified)?;
        if observed.workspace_id != runtime.workspace_id
            || observed.pane_id != runtime.pane_id
            || Some(observed.tab_id.as_str()) != runtime.tab_id.as_deref()
            || !observed.interactive_ready
        {
            return Err(OrchestratorReplacementServiceError::ReplacementUnverified);
        }
        let provider_session = observed
            .provider_session
            .clone()
            .ok_or(OrchestratorReplacementServiceError::ReplacementUnverified)?;
        if runtime
            .provider_session
            .as_ref()
            .is_some_and(|expected| expected != &provider_session)
        {
            return Err(OrchestratorReplacementServiceError::ReplacementUnverified);
        }
        runtime.provider_session = Some(provider_session);
        runtime.observation_state = RuntimeObservationState::Observed;
        runtime.process_state = RuntimeProcessState::Running;
        runtime.status = observed.status;
        runtime.state_change_sequence = observed.state_change_sequence;
        runtime.revision = observed.revision;
        runtime.last_observed_at_unix_ms = inventory.observed_at_unix_ms;
        Ok(runtime)
    }

    async fn reconcile_after_ambiguous_failure(
        &self,
        command_id: &str,
    ) -> Result<(), ProjectStoreError> {
        let recovered = self
            .store
            .recover_pending_project_orchestrator_replacement(command_id, STRANDED_PENDING_MESSAGE)
            .await;
        if let Err(error) = &recovered {
            warn!(
                command_id,
                error = %error,
                "Pending orchestrator replacement could not be promoted for live recovery"
            );
        }
        if let Err(error) = self.reconcile_ambiguous_replacement(command_id).await {
            warn!(
                command_id,
                error = %error,
                "Ambiguous orchestrator replacement recovery could not be completed; durable captures remain reserved for startup reconciliation"
            );
        }
        recovered
    }

    async fn fail_ambiguous_replacement(
        &self,
        command_id: &str,
        message: &str,
    ) -> Result<(), ProjectStoreError> {
        let result = self
            .store
            .fail_project_orchestrator_replacement(command_id, message, true)
            .await;
        let recovered = self.reconcile_after_ambiguous_failure(command_id).await;
        match (result, recovered) {
            (Ok(()), _) | (_, Ok(())) => Ok(()),
            (Err(error), Err(_)) => Err(error),
        }
    }

    async fn process_cleanup(&self, command_id: &str, result: &mut ReplacedProjectOrchestrator) {
        if !result.cleanup_pending {
            return;
        }
        match self.cleanup.process_command(command_id).await {
            Ok(report) if report.attempted > 0 => {
                result.cleanup_pending = report.failed > 0;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    command_id,
                    error = %error,
                    "Orchestrator replacement committed with cleanup pending"
                );
            }
        }
    }
}

async fn replacement_recovery_inventory(
    source: &Arc<dyn InventorySource>,
    inventories: &mut HashMap<(String, String), Result<RuntimeInventory, String>>,
    adapter: &str,
    session: &str,
) -> Result<RuntimeInventory, String> {
    let key = (adapter.to_owned(), session.to_owned());
    if let Some(inventory) = inventories.get(&key) {
        return inventory.clone();
    }
    let inventory = source
        .inventory(session)
        .await
        .map_err(|error| error.to_string());
    inventories.insert(key, inventory.clone());
    inventory
}

fn recovery_retry_after_ms(attempts: u32) -> u64 {
    let exponent = attempts.min(6);
    RECOVERY_RETRY_BASE_MS
        .saturating_mul(1_u64 << exponent)
        .min(RECOVERY_RETRY_CAP_MS)
}

#[cfg(not(test))]
const fn prepare_absence_confirmation_delay_ms() -> u64 {
    RECOVERY_RETRY_BASE_MS
}

#[cfg(test)]
const fn prepare_absence_confirmation_delay_ms() -> u64 {
    0
}

const fn outcome_requires_retry(outcome: OrchestratorReplacementRecoveryOutcome) -> bool {
    matches!(
        outcome,
        OrchestratorReplacementRecoveryOutcome::ConflictingReused
            | OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable
    )
}

fn update_recovery_report(
    report: &mut OrchestratorReplacementRecoveryReport,
    outcome: OrchestratorReplacementRecoveryOutcome,
) {
    match outcome {
        OrchestratorReplacementRecoveryOutcome::AdoptedCurrent => {}
        OrchestratorReplacementRecoveryOutcome::AbsentConverged => {
            report.absent_captures += 1;
        }
        OrchestratorReplacementRecoveryOutcome::ConflictingReused => {
            report.conflicting_captures += 1;
        }
        OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable => {
            report.quarantined_captures += 1;
        }
    }
}

enum PrepareIntentResolution {
    Prepared(Box<WorkerRuntimeBinding>),
    Absent(&'static str),
    Conflicting(&'static str),
    PresentNotSafelyRetirable(&'static str),
    Deferred(&'static str),
}

impl PrepareIntentResolution {
    fn persisted_outcome(&self) -> (OrchestratorReplacementRecoveryOutcome, &'static str) {
        match self {
            Self::Prepared(_) | Self::Deferred(_) => (
                OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable,
                "Replacement prepare intent was not fully resolved",
            ),
            Self::Absent(detail) => (
                OrchestratorReplacementRecoveryOutcome::AbsentConverged,
                detail,
            ),
            Self::Conflicting(detail) => (
                OrchestratorReplacementRecoveryOutcome::ConflictingReused,
                detail,
            ),
            Self::PresentNotSafelyRetirable(detail) => (
                OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable,
                detail,
            ),
        }
    }
}

fn resolve_prepare_intent(
    inventory: &RuntimeInventory,
    intent: &OrchestratorReplacementPrepareIntent,
) -> PrepareIntentResolution {
    let required_after = intent
        .last_absence_observed_at_unix_ms
        .unwrap_or(intent.intent_at_unix_ms);
    if inventory.observed_at_unix_ms <= required_after {
        return PrepareIntentResolution::Deferred(
            "Inventory observation is not newer than the durable replacement prepare evidence",
        );
    }
    if inventory.adapter != intent.adapter || inventory.session != intent.session {
        return PrepareIntentResolution::Conflicting(
            "Fresh inventory adapter or session did not match the replacement prepare intent",
        );
    }
    if !inventory
        .workspaces
        .iter()
        .any(|workspace| workspace.runtime_id == intent.workspace_id)
    {
        return PrepareIntentResolution::Conflicting(
            "The replacement prepare intent workspace is not present in fresh inventory",
        );
    }
    let tabs = inventory
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == intent.workspace_id && tab.label == intent.tab_label)
        .collect::<Vec<_>>();
    if tabs.is_empty() {
        return PrepareIntentResolution::Absent(
            "The command-unique replacement prepare label is absent from its exact workspace in fresh inventory",
        );
    }
    if tabs.len() > 1 {
        return PrepareIntentResolution::Conflicting(
            "The command-unique replacement prepare label matched multiple tabs in its exact workspace",
        );
    }
    let tab = tabs[0];
    let panes = inventory
        .panes
        .iter()
        .filter(|pane| pane.workspace_id == intent.workspace_id && pane.tab_id == tab.runtime_id)
        .collect::<Vec<_>>();
    if panes.len() != 1 || tab.pane_count != 1 {
        return PrepareIntentResolution::PresentNotSafelyRetirable(
            "The command-unique replacement prepare tab is present without one exact root pane; operator inspection is required",
        );
    }
    let pane = panes[0];
    let exact_workers = inventory
        .workers
        .iter()
        .filter(|worker| {
            worker.workspace_id == intent.workspace_id
                && worker.tab_id == tab.runtime_id
                && worker.pane_id == pane.runtime_id
                && worker.terminal_id == pane.terminal_id
        })
        .collect::<Vec<_>>();
    if exact_workers.len() > 1
        || inventory.workers.iter().any(|worker| {
            (worker.tab_id == tab.runtime_id
                || worker.pane_id == pane.runtime_id
                || worker.terminal_id == pane.terminal_id)
                && !exact_workers.contains(&worker)
        })
    {
        return PrepareIntentResolution::Conflicting(
            "The command-unique replacement prepare topology is partially reused by a conflicting worker identity",
        );
    }
    let worker = exact_workers.first().copied();
    if worker
        .and_then(|worker| worker.provider_session.as_ref())
        .zip(pane.provider_session.as_ref())
        .is_some_and(|(worker, pane)| worker != pane)
    {
        return PrepareIntentResolution::Conflicting(
            "The command-unique replacement prepare worker and pane disagree on provider identity",
        );
    }
    PrepareIntentResolution::Prepared(Box::new(prepared_runtime_from_intent(
        intent, inventory, tab, pane, worker,
    )))
}

fn prepared_runtime_from_intent(
    intent: &OrchestratorReplacementPrepareIntent,
    inventory: &RuntimeInventory,
    tab: &yard_domain::TabObservation,
    pane: &yard_domain::PaneObservation,
    worker: Option<&ObservedWorker>,
) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        adapter: intent.adapter.clone(),
        session: intent.session.clone(),
        workspace_id: intent.workspace_id.clone(),
        terminal_id: pane.terminal_id.clone(),
        tab_id: Some(tab.runtime_id.clone()),
        pane_id: pane.runtime_id.clone(),
        provider_session: worker
            .and_then(|worker| worker.provider_session.clone())
            .or_else(|| pane.provider_session.clone()),
        owns_tab: true,
        observation_state: RuntimeObservationState::Observed,
        process_state: if worker.is_some() {
            RuntimeProcessState::Running
        } else {
            RuntimeProcessState::Unknown
        },
        status: worker.map_or(pane.status, |worker| worker.status),
        state_change_sequence: worker.map_or(0, |worker| worker.state_change_sequence),
        revision: worker.map_or(pane.revision, |worker| worker.revision),
        version: 1,
        last_observed_at_unix_ms: inventory.observed_at_unix_ms,
    }
}

enum CaptureResolution {
    Adoptable(Box<WorkerRuntimeBinding>),
    Absent(&'static str),
    Conflicting(&'static str),
    PresentNotSafelyRetirable(&'static str),
    Deferred(&'static str),
}

impl CaptureResolution {
    fn persisted_outcome(&self) -> (OrchestratorReplacementRecoveryOutcome, &'static str) {
        match self {
            Self::Adoptable(_) => (
                OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable,
                "Confirmed started runtime is present, but it was not adopted; operator inspection is required",
            ),
            Self::Absent(detail) => (
                OrchestratorReplacementRecoveryOutcome::AbsentConverged,
                detail,
            ),
            Self::Conflicting(detail) => (
                OrchestratorReplacementRecoveryOutcome::ConflictingReused,
                detail,
            ),
            Self::PresentNotSafelyRetirable(detail) | Self::Deferred(detail) => (
                OrchestratorReplacementRecoveryOutcome::PresentNotSafelyRetirable,
                detail,
            ),
        }
    }
}

fn resolve_replacement_capture(
    inventory: &RuntimeInventory,
    capture: &OrchestratorReplacementRuntimeCapture,
) -> CaptureResolution {
    if inventory.observed_at_unix_ms <= capture.runtime.last_observed_at_unix_ms {
        return CaptureResolution::Deferred(
            "Inventory observation is not newer than the durable replacement runtime capture",
        );
    }
    if inventory.adapter != capture.runtime.adapter || inventory.session != capture.runtime.session
    {
        return CaptureResolution::Conflicting(
            "Fresh inventory adapter or session did not match the captured runtime identity",
        );
    }
    if let Some(provider_session) = capture.runtime.provider_session.as_ref() {
        return resolve_provider_capture(inventory, capture, provider_session);
    }
    resolve_topology_capture(inventory, capture)
}

fn resolve_provider_capture(
    inventory: &RuntimeInventory,
    capture: &OrchestratorReplacementRuntimeCapture,
    provider_session: &yard_domain::ProviderSessionRef,
) -> CaptureResolution {
    let provider_workers = inventory
        .workers
        .iter()
        .filter(|worker| worker.provider_session.as_ref() == Some(provider_session))
        .collect::<Vec<_>>();
    let provider_panes = inventory
        .panes
        .iter()
        .filter(|pane| pane.provider_session.as_ref() == Some(provider_session))
        .collect::<Vec<_>>();
    if provider_workers.len() > 1 {
        return CaptureResolution::Conflicting(
            "Captured provider identity was observed on multiple workers",
        );
    }
    if let Some(worker) = provider_workers.first() {
        if provider_panes.len() > 1
            || provider_panes
                .iter()
                .any(|pane| !pane_matches_capture(pane, &capture.runtime))
        {
            return CaptureResolution::Conflicting(
                "Captured provider identity was observed on conflicting worker and pane identities",
            );
        }
        if !worker_matches_capture(worker, &capture.runtime) {
            return CaptureResolution::Conflicting(
                "Captured provider identity is present with mismatched workspace, tab, pane, or terminal topology",
            );
        }
        if !inventory
            .workspaces
            .iter()
            .any(|workspace| workspace.runtime_id == capture.runtime.workspace_id)
        {
            return CaptureResolution::Conflicting(
                "Captured provider identity is present but its workspace is not observed",
            );
        }
        if capture.role == OrchestratorReplacementRuntimeRole::Started
            && capture.objective_delivery_confirmed
            && worker.interactive_ready
        {
            return CaptureResolution::Adoptable(Box::new(observed_replacement_runtime(
                &capture.runtime,
                worker,
                inventory.observed_at_unix_ms,
            )));
        }
        return CaptureResolution::PresentNotSafelyRetirable(
            "Captured provider identity is present, but durable workflow evidence or interactive readiness is insufficient for adoption and protocol 19 cannot retire it safely",
        );
    }
    if provider_panes.len() > 1 {
        return CaptureResolution::Conflicting(
            "Captured provider identity was observed on multiple panes",
        );
    }
    if let Some(pane) = provider_panes.first() {
        if pane_matches_capture(pane, &capture.runtime) {
            return CaptureResolution::PresentNotSafelyRetirable(
                "Captured provider identity remains present on the exact pane topology without an adoptable interactive worker; protocol 19 cannot retire it safely",
            );
        }
        return CaptureResolution::Conflicting(
            "Captured provider identity is present on mismatched pane topology",
        );
    }
    if topology_identity_is_reused(inventory, &capture.runtime) {
        CaptureResolution::Conflicting(
            "Captured provider identity is absent, but one or more captured topology identifiers have been reused",
        )
    } else {
        CaptureResolution::Absent(
            "Captured provider and topology identities are absent from fresh inventory; the reservation converged without destructive cleanup",
        )
    }
}

fn resolve_topology_capture(
    inventory: &RuntimeInventory,
    capture: &OrchestratorReplacementRuntimeCapture,
) -> CaptureResolution {
    if inventory
        .workers
        .iter()
        .any(|worker| worker_matches_capture(worker, &capture.runtime))
        || inventory
            .panes
            .iter()
            .any(|pane| pane_matches_capture(pane, &capture.runtime))
    {
        return CaptureResolution::PresentNotSafelyRetirable(
            "Prepared capture remains present at exact workspace, tab, pane, and terminal topology; protocol 19 cannot retire it with an atomic identity guard",
        );
    }
    if topology_identity_is_reused(inventory, &capture.runtime) {
        CaptureResolution::Conflicting(
            "Prepared capture topology is partially present or reused by a different runtime identity",
        )
    } else {
        CaptureResolution::Absent(
            "Prepared capture topology is absent from fresh inventory; the reservation converged without destructive cleanup",
        )
    }
}

fn worker_matches_capture(worker: &ObservedWorker, capture: &WorkerRuntimeBinding) -> bool {
    worker.workspace_id == capture.workspace_id
        && Some(worker.tab_id.as_str()) == capture.tab_id.as_deref()
        && worker.pane_id == capture.pane_id
        && worker.terminal_id == capture.terminal_id
}

fn pane_matches_capture(
    pane: &yard_domain::PaneObservation,
    capture: &WorkerRuntimeBinding,
) -> bool {
    pane.workspace_id == capture.workspace_id
        && Some(pane.tab_id.as_str()) == capture.tab_id.as_deref()
        && pane.runtime_id == capture.pane_id
        && pane.terminal_id == capture.terminal_id
}

fn topology_identity_is_reused(
    inventory: &RuntimeInventory,
    capture: &WorkerRuntimeBinding,
) -> bool {
    inventory.workers.iter().any(|worker| {
        worker.terminal_id == capture.terminal_id
            || worker.pane_id == capture.pane_id
            || capture
                .tab_id
                .as_deref()
                .is_some_and(|tab_id| worker.tab_id == tab_id)
    }) || inventory.panes.iter().any(|pane| {
        pane.terminal_id == capture.terminal_id
            || pane.runtime_id == capture.pane_id
            || capture
                .tab_id
                .as_deref()
                .is_some_and(|tab_id| pane.tab_id == tab_id)
    })
}

fn observed_replacement_runtime(
    capture: &WorkerRuntimeBinding,
    observed: &ObservedWorker,
    observed_at_unix_ms: u64,
) -> WorkerRuntimeBinding {
    let mut runtime = capture.clone();
    runtime
        .provider_session
        .clone_from(&observed.provider_session);
    runtime.observation_state = RuntimeObservationState::Observed;
    runtime.process_state = RuntimeProcessState::Running;
    runtime.status = observed.status;
    runtime.state_change_sequence = observed.state_change_sequence;
    runtime.revision = observed.revision;
    runtime.last_observed_at_unix_ms = observed_at_unix_ms;
    runtime
}

fn recovery_requires_operator_attention(error: &ProjectStoreError) -> bool {
    matches!(
        error,
        ProjectStoreError::OrchestratorReplacementTargetChanged
            | ProjectStoreError::OrchestratorReplacementRuntimeConflict
            | ProjectStoreError::RuntimeWorkspaceMismatch
            | ProjectStoreError::RuntimeWorkerAlreadyBound
            | ProjectStoreError::StaleRuntimeSnapshot
            | ProjectStoreError::OrchestratorInterventionInProgress
            | ProjectStoreError::OrchestratorReplacementRecoveryUnconfirmed
            | ProjectStoreError::OrchestratorReplacementFreshObservationRequired
    )
}

#[derive(Debug, Error)]
pub enum OrchestratorReplacementServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::OrchestratorReplacementValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("the project-bound runtime workspace '{0}' is not currently observed")]
    RuntimeWorkspaceMissing(String),
    #[error("the project workspace has no usable working directory")]
    RuntimeCwdUnavailable,
    #[error("the displaced orchestrator provider or topology identity changed")]
    RuntimeIdentityChanged,
    #[error("Herdr replacement provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("Herdr replacement provisioning outcome is ambiguous: {0}")]
    RuntimeProvisionAmbiguous(String),
    #[error("the prepared replacement runtime failed fresh provider/topology verification")]
    ReplacementUnverified,
    #[error("the replacement worker was created but objective delivery failed: {0}")]
    ObjectiveDeliveryFailed(String),
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap, VecDeque},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use rusqlite::Connection;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, CreateProject, CreateWorkerProfile, FocusObservation, ObservedStatus,
        ObservedWorker, OldSessionDisposition, PaneObservation, ProjectRuntimeBinding,
        ProviderSessionRef, ReplaceProjectOrchestrator, RuntimeInventory, RuntimeObservationState,
        RuntimeProcessState, RuntimeSession, RuntimeSessions, TabObservation, WorkerProfileSpec,
        WorkerRuntimeBinding, WorkspaceObservation,
    };
    use yard_store::{
        BeginProjectOrchestratorReplacement, OrchestratorReplacementPrepareIntent,
        OrchestratorReplacementRuntimeCapture, OrchestratorReplacementRuntimeRole,
        SqliteProjectStore, YardStore,
    };

    use super::{
        CaptureResolution, OrchestratorReplacementService, PrepareIntentResolution,
        STRANDED_PENDING_MESSAGE, resolve_prepare_intent, resolve_replacement_capture,
    };
    use crate::{
        allocation_service::{
            RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeRetirementError,
            RuntimeRetirementRequest,
        },
        inventory_service::{InventoryServiceError, InventorySource},
    };

    fn provider_session(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn runtime(
        terminal_id: &str,
        pane_id: &str,
        provider_session: Option<ProviderSessionRef>,
    ) -> WorkerRuntimeBinding {
        runtime_at(
            "default",
            "workspace-1",
            terminal_id,
            pane_id,
            provider_session,
        )
    }

    fn runtime_at(
        session: &str,
        workspace_id: &str,
        terminal_id: &str,
        pane_id: &str,
        provider_session: Option<ProviderSessionRef>,
    ) -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: session.to_owned(),
            workspace_id: workspace_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            tab_id: Some(format!("tab-{pane_id}")),
            pane_id: pane_id.to_owned(),
            provider_session,
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Working,
            state_change_sequence: 3,
            revision: 2,
            version: 1,
            last_observed_at_unix_ms: 10,
        }
    }

    fn observed_worker(runtime: &WorkerRuntimeBinding) -> ObservedWorker {
        ObservedWorker {
            runtime_id: runtime.terminal_id.clone(),
            terminal_id: runtime.terminal_id.clone(),
            workspace_id: runtime.workspace_id.clone(),
            tab_id: runtime.tab_id.clone().unwrap(),
            pane_id: runtime.pane_id.clone(),
            name: Some("yard-replacement".to_owned()),
            provider: Some("codex".to_owned()),
            display_provider: Some("Codex".to_owned()),
            status: ObservedStatus::Working,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 4,
            cwd: Some("/tmp/project".to_owned()),
            foreground_cwd: Some("/tmp/project".to_owned()),
            tokens: BTreeMap::new(),
            provider_session: runtime.provider_session.clone(),
            revision: 3,
        }
    }

    fn inventory(workers: Vec<ObservedWorker>) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.8.0".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 20,
            focus: FocusObservation::default(),
            workspaces: vec![WorkspaceObservation {
                runtime_id: "workspace-1".to_owned(),
                order: 0,
                label: "Project".to_owned(),
                focused: false,
                active_tab_id: "tab-pane-prepared".to_owned(),
                pane_count: workers.len(),
                tab_count: workers.len(),
                status: ObservedStatus::Working,
                tokens: BTreeMap::new(),
                worktree: None,
            }],
            tabs: Vec::new(),
            panes: Vec::new(),
            workers,
            child_agents: Vec::new(),
        }
    }

    fn prepare_intent(tab_label: &str) -> OrchestratorReplacementPrepareIntent {
        OrchestratorReplacementPrepareIntent {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            tab_label: tab_label.to_owned(),
            intent_at_unix_ms: 10,
            recovery_outcome: None,
            recovery_attempts: 0,
            absence_observations: 0,
            last_absence_observed_at_unix_ms: None,
        }
    }

    fn prepared_intent_inventory(tab_label: &str) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.8.0".to_owned(),
            protocol: 19,
            observed_at_unix_ms: i64::MAX as u64,
            focus: FocusObservation::default(),
            workspaces: vec![WorkspaceObservation {
                runtime_id: "workspace-1".to_owned(),
                order: 0,
                label: "Project".to_owned(),
                focused: false,
                active_tab_id: "tab-intent".to_owned(),
                pane_count: 1,
                tab_count: 1,
                status: ObservedStatus::Unknown,
                tokens: BTreeMap::new(),
                worktree: None,
            }],
            tabs: vec![TabObservation {
                runtime_id: "tab-intent".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                order: 0,
                label: tab_label.to_owned(),
                focused: false,
                pane_count: 1,
                status: ObservedStatus::Unknown,
            }],
            panes: vec![PaneObservation {
                runtime_id: "pane-intent".to_owned(),
                terminal_id: "terminal-intent".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                tab_id: "tab-intent".to_owned(),
                focused: false,
                cwd: Some("/tmp/project".to_owned()),
                foreground_cwd: Some("/tmp/project".to_owned()),
                label: None,
                provider: None,
                display_provider: None,
                status: ObservedStatus::Unknown,
                tokens: BTreeMap::new(),
                provider_session: None,
                revision: 1,
            }],
            workers: Vec::new(),
            child_agents: Vec::new(),
        }
    }

    fn capture(
        role: OrchestratorReplacementRuntimeRole,
        runtime: WorkerRuntimeBinding,
        objective_delivery_confirmed: bool,
    ) -> OrchestratorReplacementRuntimeCapture {
        OrchestratorReplacementRuntimeCapture {
            role,
            runtime,
            objective_delivery_confirmed,
            recovery_outcome: None,
            recovery_attempts: 0,
        }
    }

    #[test]
    fn prepare_intent_resolves_only_one_exact_unowned_root_pane() {
        let label = "Yard replacement [replace-intent]";
        let resolution =
            resolve_prepare_intent(&prepared_intent_inventory(label), &prepare_intent(label));
        let PrepareIntentResolution::Prepared(runtime) = resolution else {
            panic!("expected exact prepare intent to produce a durable runtime capture");
        };
        assert_eq!(runtime.workspace_id, "workspace-1");
        assert_eq!(runtime.tab_id.as_deref(), Some("tab-intent"));
        assert_eq!(runtime.pane_id, "pane-intent");
        assert_eq!(runtime.terminal_id, "terminal-intent");
        assert!(runtime.provider_session.is_none());
    }

    #[test]
    fn prepare_intent_proven_absence_converges() {
        let mut inventory = prepared_intent_inventory("different label");
        inventory.tabs.clear();
        inventory.panes.clear();
        assert!(matches!(
            resolve_prepare_intent(
                &inventory,
                &prepare_intent("Yard replacement [replace-intent]")
            ),
            PrepareIntentResolution::Absent(_)
        ));
    }

    #[test]
    fn prepare_intent_captures_an_exact_unverified_worker_for_quarantine() {
        let label = "Yard replacement [replace-intent-worker]";
        let mut inventory = prepared_intent_inventory(label);
        let mut started = runtime(
            "terminal-intent",
            "pane-intent",
            Some(provider_session("provider-intent")),
        );
        started.tab_id = Some("tab-intent".to_owned());
        inventory.workers = vec![observed_worker(&started)];

        let resolution = resolve_prepare_intent(&inventory, &prepare_intent(label));
        let PrepareIntentResolution::Prepared(runtime) = resolution else {
            panic!("expected exact unverified worker topology to be captured");
        };
        assert_eq!(
            runtime
                .provider_session
                .as_ref()
                .map(|provider| provider.value.as_str()),
            Some("provider-intent")
        );
        assert_eq!(runtime.process_state, RuntimeProcessState::Running);
    }

    #[test]
    fn pre_start_interruption_quarantines_present_prepared_topology() {
        let prepared = runtime("terminal-prepared", "pane-prepared", None);
        assert!(matches!(
            resolve_replacement_capture(
                &inventory(vec![observed_worker(&prepared)]),
                &capture(
                    OrchestratorReplacementRuntimeRole::Prepared,
                    prepared,
                    false,
                ),
            ),
            CaptureResolution::PresentNotSafelyRetirable(_)
        ));
    }

    #[test]
    fn post_start_pre_persist_interruption_quarantines_unconfirmed_worker() {
        let prepared = runtime("terminal-prepared", "pane-prepared", None);
        let mut started_observation = prepared.clone();
        started_observation.provider_session = Some(provider_session("provider-started"));
        assert!(matches!(
            resolve_replacement_capture(
                &inventory(vec![observed_worker(&started_observation)]),
                &capture(
                    OrchestratorReplacementRuntimeRole::Prepared,
                    prepared,
                    false,
                ),
            ),
            CaptureResolution::PresentNotSafelyRetirable(_)
        ));
    }

    #[test]
    fn started_topology_mismatch_is_preserved_for_operator_attention() {
        let started = runtime(
            "terminal-mismatch",
            "pane-mismatch",
            Some(provider_session("provider-mismatch")),
        );
        let present = resolve_replacement_capture(
            &inventory(vec![observed_worker(&started)]),
            &capture(
                OrchestratorReplacementRuntimeRole::Started,
                started.clone(),
                false,
            ),
        );
        assert!(matches!(
            present,
            CaptureResolution::PresentNotSafelyRetirable(_)
        ));
    }

    #[test]
    fn conflicting_reused_identity_is_preserved() {
        let started = runtime(
            "terminal-mismatch",
            "pane-mismatch",
            Some(provider_session("provider-mismatch")),
        );
        let mut reused = started.clone();
        reused.provider_session = Some(provider_session("different-provider"));
        reused.pane_id = "pane-reused".to_owned();
        let conflicting = resolve_replacement_capture(
            &inventory(vec![observed_worker(&reused)]),
            &capture(OrchestratorReplacementRuntimeRole::Started, started, true),
        );
        assert!(matches!(conflicting, CaptureResolution::Conflicting(_)));
    }

    #[test]
    fn confirmed_started_capture_is_adoptable() {
        let started = runtime(
            "terminal-started",
            "pane-started",
            Some(provider_session("provider-started")),
        );
        assert!(matches!(
            resolve_replacement_capture(
                &inventory(vec![observed_worker(&started)]),
                &capture(
                    OrchestratorReplacementRuntimeRole::Started,
                    started.clone(),
                    true,
                ),
            ),
            CaptureResolution::Adoptable(_)
        ));
    }

    #[test]
    fn proven_absence_converges_without_destructive_cleanup() {
        let started = runtime(
            "terminal-started",
            "pane-started",
            Some(provider_session("provider-started")),
        );
        assert!(matches!(
            resolve_replacement_capture(
                &inventory(Vec::new()),
                &capture(OrchestratorReplacementRuntimeRole::Started, started, true,),
            ),
            CaptureResolution::Absent(_)
        ));
    }

    struct SequencedInventory {
        inventories: Mutex<VecDeque<RuntimeInventory>>,
    }

    #[derive(Clone)]
    struct SelectiveInventory {
        inventories: HashMap<String, RuntimeInventory>,
        failed_session: String,
    }

    #[async_trait]
    impl InventorySource for SelectiveInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            unreachable!("replacement recovery reads only captured sessions")
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            if session_name == self.failed_session {
                return Err(InventoryServiceError::Herdr(
                    yard_herdr::HerdrError::SessionNotFound(session_name.to_owned()),
                ));
            }
            Ok(self.inventories[session_name].clone())
        }
    }

    #[async_trait]
    impl InventorySource for SequencedInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![RuntimeSession {
                    name: "default".to_owned(),
                    is_default: true,
                    running: true,
                }],
            })
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventories = self.inventories.lock().unwrap();
            if inventories.len() > 1 {
                return Ok(inventories.pop_front().unwrap());
            }
            Ok(inventories
                .front()
                .expect("sequenced inventory must contain an observation")
                .clone())
        }
    }

    #[derive(Default)]
    struct NoRetirementRuntime {
        retire_calls: AtomicUsize,
    }

    #[async_trait]
    impl RuntimeControl for NoRetirementRuntime {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            Err(RuntimeProvisionError::BeforeWorker(
                "not used by recovery".to_owned(),
            ))
        }

        async fn retire_runtime(
            &self,
            _request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            self.retire_calls.fetch_add(1, Ordering::SeqCst);
            Err(RuntimeRetirementError::AtomicIdentityGuardUnavailable)
        }
    }

    #[tokio::test]
    async fn active_registration_waits_for_stranded_recovery_sweep() {
        let temp = TempDir::new().unwrap();
        let store: Arc<dyn YardStore> = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let source: Arc<dyn InventorySource> = Arc::new(SequencedInventory {
            inventories: Mutex::new(VecDeque::new()),
        });
        let runtime: Arc<dyn RuntimeControl> = Arc::new(NoRetirementRuntime::default());
        let service = OrchestratorReplacementService::new(source, runtime, store);
        let contender = service.clone();
        let recovery = service.active_operations.recovery_gate.lock().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut registration = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            contender
                .begin_active_replacement("replacement-during-recovery-sweep")
                .await
        });

        started_rx.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut registration)
                .await
                .is_err()
        );
        assert!(
            !service
                .active_operations
                .active
                .lock()
                .unwrap()
                .contains("replacement-during-recovery-sweep")
        );

        drop(recovery);
        let active = tokio::time::timeout(Duration::from_secs(1), registration)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            service
                .active_operations
                .active
                .lock()
                .unwrap()
                .contains("replacement-during-recovery-sweep")
        );
        drop(active);
        assert!(
            !service
                .active_operations
                .active
                .lock()
                .unwrap()
                .contains("replacement-during-recovery-sweep")
        );
    }

    fn profile_spec() -> WorkerProfileSpec {
        WorkerProfileSpec {
            name: "Replacement".to_owned(),
            runtime_adapter: "herdr".to_owned(),
            provider: "codex".to_owned(),
            model: None,
            default_role: "orchestrator".to_owned(),
            instructions_ref: None,
            tools: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            sandbox_policy: "runtime_default".to_owned(),
            worktree_policy: "project_workspace".to_owned(),
            permission_policy: "runtime_default".to_owned(),
            completion_contract: "manual_receipt".to_owned(),
        }
    }

    async fn pending_prepared_replacement(
        store: &SqliteProjectStore,
        command_id: &str,
        session: &str,
        workspace_id: &str,
    ) -> ReplaceProjectOrchestrator {
        let displaced = runtime_at(
            session,
            workspace_id,
            &format!("terminal-displaced-{command_id}"),
            &format!("pane-displaced-{command_id}"),
            Some(provider_session(&format!(
                "provider-displaced-{command_id}"
            ))),
        );
        let project = store
            .create_project(
                CreateProject {
                    name: format!("Project {command_id}"),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: session.to_owned(),
                        workspace_id: workspace_id.to_owned(),
                    },
                    orchestrator_observed_worker_id: displaced.terminal_id.clone(),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                displaced,
            )
            .await
            .unwrap();
        let mut spec = profile_spec();
        spec.name = format!("Replacement {command_id}");
        let profile = store
            .create_worker_profile(CreateWorkerProfile { spec })
            .await
            .unwrap();
        let command = ReplaceProjectOrchestrator {
            command_id: command_id.to_owned(),
            actor: "test".to_owned(),
            expected_project_version: project.version,
            expected_orchestrator_worker_id: project.orchestrator.id.clone(),
            expected_orchestrator_worker_version: project.orchestrator.version,
            expected_orchestrator_runtime: project.orchestrator.runtime.clone().unwrap(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            objective: "Continue orchestration.".to_owned(),
            role: "orchestrator".to_owned(),
            old_session_disposition: OldSessionDisposition::RetainForInspection,
            handoff_artifact_ref: None,
        };
        store
            .begin_project_orchestrator_replacement(&project.id, command.clone())
            .await
            .unwrap();
        store
            .claim_project_orchestrator_replacement_runtime(
                command_id,
                runtime_at(
                    session,
                    workspace_id,
                    &format!("terminal-prepared-{command_id}"),
                    &format!("pane-prepared-{command_id}"),
                    None,
                ),
            )
            .await
            .unwrap();
        command
    }

    async fn ambiguous_prepared_replacement(
        store: &SqliteProjectStore,
        command_id: &str,
        session: &str,
        workspace_id: &str,
    ) -> ReplaceProjectOrchestrator {
        let command = pending_prepared_replacement(store, command_id, session, workspace_id).await;
        store
            .fail_project_orchestrator_replacement(command_id, "interrupted before start", true)
            .await
            .unwrap();
        command
    }

    #[tokio::test]
    async fn live_recovery_acknowledges_ambiguity_when_initial_persistence_fails() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let command = pending_prepared_replacement(
            &store,
            "replace-live-ambiguity-ack",
            "default",
            "workspace-1",
        )
        .await;
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_initial_replacement_ambiguity
                 BEFORE UPDATE OF status ON command_acknowledgements
                 WHEN OLD.id = 'replace-live-ambiguity-ack'
                      AND NEW.status = 'ambiguous'
                      AND NEW.error_message = 'lost external acknowledgement'
                 BEGIN
                     SELECT RAISE(FAIL, 'injected initial ambiguity persistence failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        let source = Arc::new(SequencedInventory {
            inventories: Mutex::new(VecDeque::from([inventory(Vec::new())])),
        });
        let runtime = Arc::new(NoRetirementRuntime::default());
        let service = OrchestratorReplacementService::new(source, runtime, store);

        service
            .fail_ambiguous_replacement(&command.command_id, "lost external acknowledgement")
            .await
            .unwrap();

        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let (status, error_message): (String, String) = connection
            .query_row(
                "SELECT status, error_message
                   FROM command_acknowledgements
                  WHERE id = ?1",
                [&command.command_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "ambiguous");
        assert_eq!(error_message, STRANDED_PENDING_MESSAGE);
    }

    #[tokio::test]
    async fn one_inventory_failure_defers_only_its_capture_with_durable_backoff() {
        let temp = TempDir::new().unwrap();
        let database_path = temp.path().join("yard.sqlite3");
        let store = Arc::new(SqliteProjectStore::open(&database_path).await.unwrap());
        let failed = ambiguous_prepared_replacement(
            &store,
            "replace-inventory-failed",
            "failed-session",
            "failed-workspace",
        )
        .await;
        let converged = ambiguous_prepared_replacement(
            &store,
            "replace-inventory-converged",
            "healthy-session",
            "healthy-workspace",
        )
        .await;
        let mut healthy = inventory(Vec::new());
        healthy.session = "healthy-session".to_owned();
        healthy.workspaces[0].runtime_id = "healthy-workspace".to_owned();
        let source = Arc::new(SelectiveInventory {
            inventories: HashMap::from([("healthy-session".to_owned(), healthy)]),
            failed_session: "failed-session".to_owned(),
        });
        let runtime = Arc::new(NoRetirementRuntime::default());
        let service = OrchestratorReplacementService::new(source, runtime, store.clone());

        let report = service.reconcile_ambiguous_replacements().await.unwrap();

        assert_eq!(report.absent_captures, 1);
        assert_eq!(report.deferred_items, 1);
        assert!(
            store
                .list_ambiguous_orchestrator_replacement_recoveries(Some(&converged.command_id))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .list_ambiguous_orchestrator_replacement_recoveries(Some(&failed.command_id))
                .await
                .unwrap()
                .is_empty()
        );
        drop(service);
        drop(store);

        let reopened = SqliteProjectStore::open(database_path).await.unwrap();
        assert!(
            reopened
                .list_ambiguous_orchestrator_replacement_recoveries(Some(&failed.command_id))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn interrupted_prepare_intent_is_captured_without_unsafe_close_or_global_blockage() {
        let temp = TempDir::new().unwrap();
        let database_path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&database_path).await.unwrap();
        let displaced = runtime(
            "terminal-displaced",
            "pane-displaced",
            Some(provider_session("provider-displaced")),
        );
        let project = store
            .create_project(
                CreateProject {
                    name: "Project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                    },
                    orchestrator_observed_worker_id: displaced.terminal_id.clone(),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                displaced,
            )
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec(),
            })
            .await
            .unwrap();
        let command = ReplaceProjectOrchestrator {
            command_id: "recover-present-prepared".to_owned(),
            actor: "test".to_owned(),
            expected_project_version: project.version,
            expected_orchestrator_worker_id: project.orchestrator.id.clone(),
            expected_orchestrator_worker_version: project.orchestrator.version,
            expected_orchestrator_runtime: project.orchestrator.runtime.clone().unwrap(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            objective: "Continue orchestration.".to_owned(),
            role: "orchestrator".to_owned(),
            old_session_disposition: OldSessionDisposition::RetainForInspection,
            handoff_artifact_ref: None,
        };
        let prepare_tab_label = match store
            .begin_project_orchestrator_replacement(&project.id, command.clone())
            .await
            .unwrap()
        {
            BeginProjectOrchestratorReplacement::Started(context) => context.prepare_tab_label,
            BeginProjectOrchestratorReplacement::Replayed(_) => panic!("unexpected replay"),
        };
        drop(store);

        let store = Arc::new(SqliteProjectStore::open(&database_path).await.unwrap());
        let runtime_control = Arc::new(NoRetirementRuntime::default());
        let present = prepared_intent_inventory(&prepare_tab_label);
        let mut initially_absent = present.clone();
        initially_absent.observed_at_unix_ms -= 1;
        initially_absent.tabs.clear();
        initially_absent.panes.clear();
        initially_absent.workspaces[0].pane_count = 0;
        initially_absent.workspaces[0].tab_count = 0;
        let source = Arc::new(SequencedInventory {
            inventories: Mutex::new(VecDeque::from([
                initially_absent.clone(),
                initially_absent,
                present,
            ])),
        });
        let service =
            OrchestratorReplacementService::new(source, runtime_control.clone(), store.clone());
        let first = service.reconcile_ambiguous_replacements().await.unwrap();

        assert_eq!(first.prepare_intents, 1);
        assert_eq!(first.deferred_items, 1);
        assert_eq!(first.absent_captures, 0);
        assert_eq!(first.quarantined_captures, 0);
        let second = service.reconcile_ambiguous_replacements().await.unwrap();
        assert_eq!(second.prepare_intents, 1);
        assert_eq!(second.deferred_items, 1);
        assert_eq!(second.absent_captures, 0);
        assert_eq!(second.quarantined_captures, 0);
        let third = service.reconcile_ambiguous_replacements().await.unwrap();
        assert_eq!(third.prepare_intents, 1);
        assert_eq!(third.quarantined_captures, 1);
        assert_eq!(runtime_control.retire_calls.load(Ordering::SeqCst), 0);
        let replay = service.reconcile_ambiguous_replacements().await.unwrap();
        assert_eq!(replay.prepare_intents, 0);
        assert_eq!(replay.captures, 0);

        let recovered = WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: "terminal-intent".to_owned(),
            tab_id: Some("tab-intent".to_owned()),
            pane_id: "pane-intent".to_owned(),
            provider_session: None,
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Unknown,
            status: ObservedStatus::Unknown,
            state_change_sequence: 0,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: i64::MAX as u64,
        };
        let mut colliding = command.clone();
        colliding.command_id = "replace-after-intent-collision".to_owned();
        store
            .begin_project_orchestrator_replacement(&project.id, colliding.clone())
            .await
            .unwrap();
        assert!(
            store
                .claim_project_orchestrator_replacement_runtime(&colliding.command_id, recovered,)
                .await
                .is_err()
        );
        store
            .fail_project_orchestrator_replacement(
                &colliding.command_id,
                "expected collision",
                false,
            )
            .await
            .unwrap();

        let mut unrelated = command;
        unrelated.command_id = "replace-after-intent-quarantine".to_owned();
        store
            .begin_project_orchestrator_replacement(&project.id, unrelated.clone())
            .await
            .unwrap();
        store
            .claim_project_orchestrator_replacement_runtime(
                &unrelated.command_id,
                runtime("terminal-unrelated", "pane-unrelated", None),
            )
            .await
            .unwrap();
    }
}
