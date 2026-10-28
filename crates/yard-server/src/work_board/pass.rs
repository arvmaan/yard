//! The optional model stage of a generation: tails for ambiguous items,
//! cached results by content hash, capped batches that upgrade cards in
//! place as they finish, and a hard deadline.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use yard_domain::{AssignmentLifecycle, TerminalOutputFormat};

use super::board::{self, Built, Inputs, Item, Previous};
use super::model::{self, Evidence};
use super::rules::{Column, Rule, Section};
use super::{
    BoardState, MODEL_DEADLINE, ModelSummary, READ_CONCURRENCY, READ_TIMEOUT, Stage,
    WorkBoardService, elapsed_ms, lock,
};

/// Total evidence bytes the batches may carry (keeps input under 40k tokens).
const EVIDENCE_BUDGET_BYTES: usize = 120_000;

/// Tail bytes per item: fixed, so adding an item never changes another
/// item's evidence (and cache key).
pub(crate) const TAIL_BYTES_PER_ITEM: usize = {
    let share = EVIDENCE_BUDGET_BYTES / model::MAX_MODEL_ITEMS;
    if share < model::MAX_TAIL_BYTES {
        share
    } else {
        model::MAX_TAIL_BYTES
    }
};

/// The tail reads get their own budget; the model deadline starts after them.
pub(crate) const TAILS_DEADLINE: Duration = Duration::from_secs(3);

/// Model time left after the tails, so the whole pass stays near
/// [`MODEL_DEADLINE`].
const MODEL_STAGE: Duration = MODEL_DEADLINE.saturating_sub(TAILS_DEADLINE);

/// Model batches in flight at once: all of them (a cap below
/// [`model::MAX_BATCHES`] would make the last batch wait a whole Converse
/// round trip inside [`MODEL_STAGE`]). The batch count is already bounded.
pub(crate) const MODEL_CONCURRENCY: usize = super::model::MAX_BATCHES;
const _: () = assert!(MODEL_CONCURRENCY >= super::model::MAX_BATCHES);

const REPORT_CHARS: usize = 300;
const BLOCKER_CHARS: usize = 160;
const MAX_REPORT_BLOCKERS: usize = 3;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn is_ambiguous(item: &Item) -> bool {
    item.base.rule == Rule::R5 && item.base.section == Some(Section::Ambiguous)
}

/// Cards the model writes for: every card on the main board, plus the
/// ambiguous stale ones (whose verdict can move them). Never helpers,
/// project-header orchestrators or closed records.
pub(crate) fn model_candidate(item: &Item) -> bool {
    !item.rollup
        && item.helper.is_none()
        && item.column != Column::Closed
        && (board::on_main(item) || is_ambiguous(item))
}

/// Send order: main-board cards first (Needs you, Working, Gone quiet,
/// Finished), then ambiguous Older-work cards stalest first, so a
/// cost-cap trim drops the least visible cards.
fn send_rank(item: &Item) -> (u8, u8, std::cmp::Reverse<u64>) {
    let column = match item.column {
        Column::NeedsYou => 0,
        Column::Doing => 1,
        Column::Stale => 2,
        Column::Close => 3,
        Column::Closed => 4,
    };
    (
        u8::from(!board::on_main(item)),
        column,
        std::cmp::Reverse(item.age_ms),
    )
}

/// "Needs you" / "Working now" / … for the model's `board_place`.
fn place_words(item: &Item) -> &'static str {
    match (item.column, item.section) {
        (Column::NeedsYou, _) => "Needs you",
        (Column::Doing, _) => "Working now",
        (Column::Stale, Some(Section::LikelyDone)) => "Gone quiet, probably finished",
        (Column::Stale, _) => "Gone quiet",
        (Column::Close, _) => "Finished, can be closed",
        (Column::Closed, _) => "Closed",
    }
}

/// The deterministic facts an item's cached verdict depends on: lifecycle,
/// availability, Herdr status, runtime sequence, tail revision, the project's
/// report command and snapshot id. Age is in the evidence (in days).
pub(crate) fn facts_key(inputs: &Inputs, item: &Item) -> String {
    let report = item
        .project_id
        .as_deref()
        .and_then(|id| inputs.reports.get(id))
        .map(|report| report.command_id.as_str());
    let snapshot = item
        .project_id
        .as_deref()
        .and_then(|id| inputs.snapshots.get(id))
        .map(|snapshot| snapshot.snapshot_id.as_str());
    format!(
        "{:?}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}",
        item.facts.lifecycle,
        item.facts.availability,
        item.facts.herdr,
        item.facts.worker_live,
        item.facts.has_receipt,
        item.mark.state_change_sequence,
        item.mark.tail_revision,
        report,
        snapshot,
    )
}

/// A content hash of the item's facts and everything sent for it (plus the
/// model id): an unchanged item is answered from the cache and never re-sent.
pub(crate) fn fingerprint(model_id: &str, facts_key: &str, evidence: &Evidence) -> String {
    let digest =
        Sha256::digest(format!("{model_id}\n{facts_key}\n{}", evidence.to_json()).as_bytes());
    digest[..16].iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// The evidence of one item, redacted and bounded.
pub(crate) fn evidence(inputs: &Inputs, item: &Item, tail: Option<&str>) -> Evidence {
    let objective = item
        .assignment_id
        .as_deref()
        .and_then(|id| {
            inputs
                .assignments
                .iter()
                .find(|assignment| assignment.id == id)
        })
        .map_or_else(
            || item.objective.clone(),
            |assignment| model::bounded_objective(&assignment.objective),
        );
    let snapshot_next = item
        .project_id
        .as_deref()
        .and_then(|id| inputs.snapshots.get(id))
        .map(|snapshot| {
            snapshot
                .lines
                .iter()
                .take(5)
                .map(|line| line.text.clone())
                .collect()
        })
        .unwrap_or_default();
    // The project's latest status report, through the same redaction as
    // card text. It has no timestamp of its own (see the notes).
    let report = item
        .project_id
        .as_deref()
        .and_then(|id| inputs.reports.get(id))
        .map(|report| {
            json!({
                "state": serde_json::to_value(report.state).unwrap_or_default(),
                "last": crate::redact::clean_text(&report.last, REPORT_CHARS),
                "next": crate::redact::clean_text(&report.next, REPORT_CHARS),
                "blockers": report
                    .blockers
                    .iter()
                    .filter(|blocker| !blocker.trim().is_empty())
                    .take(MAX_REPORT_BLOCKERS)
                    .map(|blocker| crate::redact::clean_text(blocker, BLOCKER_CHARS))
                    .collect::<Vec<_>>(),
            })
        });
    Evidence {
        id: item.id.clone(),
        title: item.title.clone(),
        objective,
        status: item.status_line.clone(),
        age_days: item.age_ms / DAY_MS,
        report,
        receipt_blockers: if item.kind == super::rules::ItemKind::Assignment {
            item.next_actions.clone()
        } else {
            Vec::new()
        },
        snapshot_next,
        tail: tail
            .map(|tail| model::scrub_tail(tail, TAIL_BYTES_PER_ITEM))
            .unwrap_or_default(),
        place: place_words(item).to_owned(),
        needs_title: item.title_source == super::titles::UNTITLED
            || item.title_source == super::titles::FROM_AI,
        digest: model::scrub_digest(&item.digest),
    }
}

/// How many items are sent, their batches, and each batch's
/// `(input tokens, output tokens, usd)` estimate.
pub(crate) type Plan = (usize, Vec<Vec<Evidence>>, Vec<(u64, u64, f64)>);

/// Batches of the largest prefix of `to_send` (stalest first) whose
/// pre-flight estimate fits every cap, with each batch's estimate.
pub(crate) fn affordable(
    budget: &model::Budget,
    model_id: &str,
    now_ms: u64,
    to_send: &[Evidence],
) -> Result<Plan, model::CapError> {
    let mut count = to_send.len();
    loop {
        let batches = to_send[..count]
            .chunks(model::MAX_BATCH_ITEMS)
            .map(<[Evidence]>::to_vec)
            .collect::<Vec<_>>();
        let estimates = batches
            .iter()
            .map(|batch| {
                model::estimate(
                    model_id,
                    model::user_message(now_ms, batch).len(),
                    1,
                    batch.len(),
                )
            })
            .collect::<Vec<_>>();
        let (input, output, usd) = estimates.iter().fold((0, 0, 0.0), |sum, one| {
            (sum.0 + one.0, sum.1 + one.1, sum.2 + one.2)
        });
        match budget.check(input, output, usd) {
            Ok(()) => return Ok((count, batches, estimates)),
            Err(_) if count > 1 => count -= 1,
            Err(cap) => return Err(cap),
        }
    }
}

impl WorkBoardService {
    /// Bounded tail reads for the given items: the live pane for an active
    /// assignment, else its captured transcript. `(text, revision)`.
    async fn tails(&self, items: &[&Item]) -> HashMap<String, (String, Option<u64>)> {
        let jobs = items
            .iter()
            .filter_map(|item| {
                Some((
                    item.id.clone(),
                    item.project_id.clone()?,
                    item.assignment_id.clone()?,
                    item.facts.lifecycle == Some(AssignmentLifecycle::Active),
                ))
            })
            .collect::<Vec<_>>();
        let interventions = self.inner.interventions.clone();
        let store = std::sync::Arc::clone(&self.inner.store);
        let reads = futures_util::stream::iter(jobs)
            .map(move |(id, project_id, assignment_id, live)| {
                let interventions = interventions.clone();
                let store = std::sync::Arc::clone(&store);
                async move {
                    if live
                        && let Ok(Ok(output)) = tokio::time::timeout(
                            READ_TIMEOUT,
                            interventions.read_output(
                                &project_id,
                                &assignment_id,
                                model::TAIL_LINES,
                                TerminalOutputFormat::Text,
                            ),
                        )
                        .await
                    {
                        return Some((id, (output.text, Some(output.revision))));
                    }
                    let transcript = tokio::time::timeout(
                        READ_TIMEOUT,
                        store.get_assignment_transcript(&project_id, &assignment_id),
                    )
                    .await
                    .ok()?
                    .ok()?;
                    let text = transcript.text?;
                    let lines = text.lines().collect::<Vec<_>>();
                    let start = lines.len().saturating_sub(model::TAIL_LINES as usize);
                    Some((id, (lines[start..].join("\n"), None)))
                }
            })
            .buffer_unordered(READ_CONCURRENCY);
        // Keep every read that finished by the deadline; the rest are dropped.
        let deadline = tokio::time::Instant::now() + TAILS_DEADLINE;
        let mut reads = std::pin::pin!(reads);
        let mut tails = HashMap::new();
        while let Ok(Some(result)) = tokio::time::timeout_at(deadline, reads.next()).await {
            if let Some((id, tail)) = result {
                tails.insert(id, tail);
            }
        }
        tails
    }
    #[allow(clippy::too_many_lines)]
    pub(super) async fn model_pass(
        &self,
        generation_id: &str,
        inputs: &mut Inputs,
        previous: Option<&Previous>,
        built: Built,
        warnings: &[String],
        started_ms: u64,
    ) {
        let clock = Instant::now();
        let Some(config) = self.inner.settings.model.clone() else {
            return;
        };
        inputs.triage = self.inner.triage.snapshot();
        let mut summary = ModelSummary {
            enabled: true,
            model_id: crate::redact::clean_text(&config.model_id, 80),
            ..ModelSummary::default()
        };
        let mut candidates = built
            .items
            .iter()
            .filter(|item| model_candidate(item))
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            send_rank(a)
                .cmp(&send_rank(b))
                .then_with(|| a.id.cmp(&b.id))
        });
        let ambiguous = candidates
            .iter()
            .take(model::MAX_MODEL_ITEMS)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        if ambiguous.is_empty() {
            summary.skipped = true;
            self.finish_model(generation_id, built, summary, warnings, clock);
            return;
        }
        // Cards stay visible with deterministic text and shimmer while the
        // tails and the model run.
        let mut pending = built.clone();
        for item in &mut pending.items {
            item.pending = ambiguous.contains(&item.id);
        }
        self.publish_built(
            generation_id,
            pending,
            Stage::Tails,
            warnings,
            BoardState::Running,
        );

        let targets = built
            .items
            .iter()
            .filter(|item| ambiguous.contains(&item.id))
            .collect::<Vec<_>>();
        let tails = self.tails(&targets).await;
        for (id, (_, revision)) in &tails {
            if let Some(revision) = revision {
                inputs.tail_revisions.insert(id.clone(), *revision);
            }
        }
        let mut built = board::build(inputs, previous);
        lock(&self.inner.run).previous = Some(Previous {
            generated_at_ms: started_ms,
            marks: board::marks(&built.items),
        });
        let mut to_send = Vec::new();
        let mut keys = HashMap::new();
        for id in &ambiguous {
            let Some(item) = built.items.iter_mut().find(|item| &item.id == id) else {
                continue;
            };
            let evidence = evidence(
                inputs,
                item,
                tails.get(&item.id).map(|(text, _)| text.as_str()),
            );
            let key = fingerprint(&config.model_id, &facts_key(inputs, item), &evidence);
            if let Some(verdict) = lock(&self.inner.lru).get(&key) {
                verdict.apply(item);
                summary.cached_items += 1;
            } else {
                item.pending = true;
                keys.insert(item.id.clone(), key);
                to_send.push(evidence);
            }
        }
        if to_send.is_empty() {
            summary.skipped = true;
            let built = board::finish(inputs, built.items);
            self.finish_model(generation_id, built, summary, warnings, clock);
            return;
        }
        // Send the largest stalest-first prefix that fits the caps; the rest
        // keep the deterministic tiebreak and are reported as skipped.
        let (count, batches, estimates) =
            match affordable(&self.inner.budget, &config.model_id, started_ms, &to_send) {
                Ok(plan) => plan,
                Err(cap) => {
                    summary.error = Some(cap.to_string());
                    summary.skipped_items = to_send.len();
                    let mut items = built.items;
                    for item in &mut items {
                        item.pending = false;
                    }
                    let built = board::finish(inputs, items);
                    self.finish_model(generation_id, built, summary, warnings, clock);
                    return;
                }
            };
        if count < to_send.len() {
            summary.skipped_items = to_send.len() - count;
            for skipped in &to_send[count..] {
                if let Some(item) = built.items.iter_mut().find(|item| item.id == skipped.id) {
                    item.pending = false;
                }
            }
            to_send.truncate(count);
        }
        // Reserve the estimate before any process starts; each batch that
        // reports usage settles to its real cost. A batch that fails without
        // usage, or is cut off in flight, stays charged at its estimate.
        let reserved = estimates.iter().map(|estimate| estimate.2).sum::<f64>();
        self.inner.budget.add(reserved);
        summary.sent_items = to_send.len();
        summary.batches_total = batches.len();
        self.publish_built(
            generation_id,
            built.clone(),
            Stage::Model,
            warnings,
            BoardState::Running,
        );
        self.publish(|board| {
            board.mode = "model";
            board.model = Some(summary.clone());
        });

        // Batches run concurrently (at most MODEL_CONCURRENCY at a time);
        // the runner shares one credential export and account check.
        let jobs = batches
            .iter()
            .cloned()
            .zip(estimates.iter().map(|estimate| estimate.2))
            .map(|(batch, estimate)| {
                let config = config.clone();
                async move {
                    (
                        model::run_batch(&config, started_ms, &batch).await,
                        estimate,
                    )
                }
            })
            .collect::<Vec<_>>();
        let running = futures_util::stream::iter(jobs).buffer_unordered(MODEL_CONCURRENCY);
        let mut running = std::pin::pin!(running);
        let deadline = tokio::time::Instant::now() + MODEL_STAGE;
        let mut errors = Vec::new();
        let mut items = built.items;
        loop {
            let next = tokio::time::timeout_at(deadline, running.next()).await;
            let (outcome, estimate) = match next {
                Ok(Some(done)) => done,
                Ok(None) => break,
                Err(_) => {
                    errors.push(format!(
                        "model pass stopped after {} s; kept {} of {} batches",
                        MODEL_STAGE.as_secs(),
                        summary.batches_done,
                        summary.batches_total
                    ));
                    break;
                }
            };
            summary.calls += 1;
            summary.batches_done += 1;
            summary.input_tokens += outcome.input_tokens;
            summary.output_tokens += outcome.output_tokens;
            if outcome.measured {
                summary.usd += outcome.usd;
                self.inner.budget.add(outcome.usd - estimate);
            } else {
                summary.usd += estimate;
            }
            if let Some(error) = outcome.error {
                errors.push(error);
            }
            for item in &mut items {
                if let Some(verdict) = outcome.verdicts.get(&item.id) {
                    verdict.apply(item);
                    if let Some(key) = keys.get(&item.id) {
                        lock(&self.inner.lru).put(key.clone(), verdict.clone());
                    }
                }
            }
            let partial = board::finish(inputs, items.clone());
            summary.error = errors.first().cloned();
            self.publish_built(
                generation_id,
                partial,
                Stage::Model,
                warnings,
                BoardState::Running,
            );
            self.publish(|board| board.model = Some(summary.clone()));
        }
        // Whatever did not arrive keeps the deterministic tiebreak.
        for item in &mut items {
            item.pending = false;
        }
        summary.error = errors.first().cloned();
        let built = board::finish(inputs, items);
        self.finish_model(generation_id, built, summary, warnings, clock);
    }

    fn finish_model(
        &self,
        generation_id: &str,
        built: Built,
        summary: ModelSummary,
        warnings: &[String],
        clock: Instant,
    ) {
        self.publish_built(
            generation_id,
            built,
            Stage::Done,
            warnings,
            BoardState::Idle,
        );
        let model_ms = elapsed_ms(clock);
        self.publish(|board| {
            board.mode = "model";
            board.model = Some(summary);
            board.timings.model_ms = Some(model_ms);
        });
    }
}
