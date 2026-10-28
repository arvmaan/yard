//! The model pass with a fake runner: merge, cache, caps, validation,
//! fallback and redaction of everything sent and shown.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use serde_json::{Value, json};
use yard_domain::{RuntimeInventory, RuntimeSessions};

use super::model::{self, Budget, CapError, Evidence, Lru};
use super::pass::{evidence, facts_key, fingerprint};
use super::rules::{Column, Section, Verdict};
use super::tests::board_tests::fixture;
use super::{StartError, WorkBoardService, WorkBoardSettings, board};
use crate::intervention_service::{
    InterventionService, RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest,
    RuntimeOutputResult, RuntimePromptRequest, RuntimePromptResult,
};
use crate::inventory_service::{InventoryServiceError, InventorySource};
use crate::model::{ConverseRunner, ModelError};

struct NoHerdr;

#[async_trait]
impl InventorySource for NoHerdr {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
        Ok(RuntimeSessions {
            adapter: "herdr".to_owned(),
            sessions: Vec::new(),
        })
    }

    async fn inventory(&self, _session: &str) -> Result<RuntimeInventory, InventoryServiceError> {
        unreachable!("no sessions")
    }
}

#[async_trait]
impl RuntimeIntervention for NoHerdr {
    async fn prompt(
        &self,
        _request: RuntimePromptRequest,
    ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
        Err(RuntimeInterventionError::Unavailable("test".to_owned()))
    }

    async fn read_output(
        &self,
        _request: RuntimeOutputRequest,
    ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
        Err(RuntimeInterventionError::Unavailable("test".to_owned()))
    }
}

type Reply = Box<dyn Fn(&[String]) -> Result<Value, ModelError> + Send + Sync>;

/// Records every request and answers from `reply(ids in the batch)`.
struct FakeRunner {
    calls: AtomicUsize,
    requests: Mutex<Vec<String>>,
    reply: Reply,
}

impl FakeRunner {
    fn new(
        reply: impl Fn(&[String]) -> Result<Value, ModelError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            reply: Box::new(reply),
        })
    }
}

fn batch_ids(request: &Value) -> Vec<String> {
    let text = request
        .pointer("/messages/0/content/0/text")
        .and_then(Value::as_str)
        .unwrap();
    let json_start = text.find("\n\n{").unwrap() + 2;
    let body: Value = serde_json::from_str(&text[json_start..]).unwrap();
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[async_trait]
impl ConverseRunner for FakeRunner {
    async fn converse(&self, request: &Value) -> Result<Value, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.to_string());
        (self.reply)(&batch_ids(request))
    }
}

fn converse_reply(text: &str, stop: &str) -> Value {
    json!({
        "stopReason": stop,
        "usage": {"inputTokens": 2_000, "outputTokens": 300},
        "output": {"message": {"content": [{"text": text}]}},
    })
}

fn secrets() -> Vec<String> {
    // Built at runtime so secret scanners do not flag fake values.
    vec![
        format!("AKIA{}", "QWERTYUIOPASDFGH"),
        format!(
            "{}{}",
            "eyJhbGciOi", "JIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl"
        ),
        "hunter2-password-value".to_owned(),
        "/Users/someone/private/repo".to_owned(),
        "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7".to_owned(),
    ]
}

fn planted_objective() -> String {
    let s = secrets();
    format!(
        "Rotate {} and call with Bearer {} then PASSWORD={} in {}/x.rs\n{}\n{}\n{}",
        s[0],
        s[1],
        s[2],
        s[3],
        pem_marker("BEGIN"),
        s[4],
        pem_marker("END")
    )
}

/// A PEM armour line, built at runtime so secret scanners stay quiet.
fn pem_marker(kind: &str) -> String {
    ["-----", kind, " PRIVATE", " KEY-----"].concat()
}

async fn make_service(settings: WorkBoardSettings) -> (WorkBoardService, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let store: Arc<dyn yard_store::YardStore> = Arc::new(
        yard_store::SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
            .await
            .unwrap(),
    );
    let source: Arc<dyn InventorySource> = Arc::new(NoHerdr);
    let interventions =
        InterventionService::new(Arc::clone(&source), Arc::new(NoHerdr), Arc::clone(&store));
    let service = WorkBoardService::new(
        store,
        source,
        interventions,
        temp.path().join("knowledge"),
        settings,
    );
    (service, temp)
}

/// Run the model stage over the neutral fixture (with secrets planted in a
/// stale item's objective) and return the published board.
async fn run_pass(service: &WorkBoardService) -> super::Board {
    let mut inputs = fixture();
    inputs
        .assignments
        .iter_mut()
        .find(|assignment| assignment.id == "a-old")
        .unwrap()
        .objective = planted_objective();
    let built = board::build(&inputs, None);
    service.publish(|board| board.generation_id = Some("g".to_owned()));
    let now = inputs.now_ms;
    service
        .model_pass("g", &mut inputs, None, built, &[], now)
        .await;
    service.board()
}

fn verdicts_for(ids: &[String]) -> String {
    let s = secrets();
    let entries = ids
        .iter()
        .map(|id| match id.as_str() {
            "a:a-old" => json!({
                "id": id, "verdict": "finished", "confidence": 0.82,
                "one_line": format!("PR merged, key {} rotated", s[0]),
                "next_actions": [{"text": "Close the docs branch", "source_id": id}, {"text": "uncited", "source_id": "nope"}],
                "evidence": "objective", "quote": format!("see {}", s[3]),
                "about": "Docs pass for the release notes",
                "status": format!("Merged; branch at /home/x/{}", s[4]),
                "your_move": "Close it if the docs look right",
            }),
            "w:w-spare" => json!({"id": id, "verdict": "unclear", "confidence": 0.9, "one_line": "", "next_actions": [], "evidence": "tail", "quote": ""}),
            _ => json!({"id": id, "verdict": "needs_work", "confidence": 0.7, "one_line": "Still open", "next_actions": [], "evidence": "objective", "quote": ""}),
        })
        .chain(std::iter::once(json!({"id": "unknown-id", "verdict": "finished", "confidence": 1.0})))
        .collect::<Vec<_>>();
    format!("```json\n{}\n```", Value::Array(entries))
}

#[tokio::test]
async fn model_pass_merges_verdicts_caches_them_and_redacts_everything() {
    let runner = FakeRunner::new(|ids| Ok(converse_reply(&verdicts_for(ids), "end_turn")));
    let (service, _temp) = make_service(WorkBoardSettings::with_model(
        "claude-haiku-4-5",
        runner.clone(),
    ))
    .await;
    let board = run_pass(&service).await;
    let calls = runner.calls.load(Ordering::SeqCst);
    assert!(calls >= 1);
    let built = board.built.as_ref().unwrap();
    let old = built
        .items
        .iter()
        .find(|item| item.id == "a:a-old")
        .unwrap();
    assert_eq!(
        (old.column, old.section),
        (Column::Stale, Some(Section::LikelyDone))
    );
    let ai = old.ai.as_ref().unwrap();
    assert_eq!(ai.verdict, Verdict::Finished);
    // The AI's lines replace the templates; a line carrying a path is dropped.
    assert_eq!(old.about, "Docs pass for the release notes");
    assert_eq!(old.your_move, "Close it if the docs look right");
    assert_eq!(old.status, super::plain::status(old));
    assert_eq!(ai.next_actions, vec!["Close the docs branch".to_owned()]);
    let spare = built
        .items
        .iter()
        .find(|item| item.id == "w:w-spare")
        .unwrap();
    assert!(spare.chips.contains(&"?".to_owned()));
    // Nine days quiet: in Older work, so it makes no to-do (the review
    // flow offers it instead).
    assert_eq!(old.relevance, super::board::Relevance::Older);
    assert!(
        !built
            .todos
            .iter()
            .any(|todo| todo.sources.iter().any(|source| source.id == old.id)),
        "{:?}",
        built.todos
    );
    let summary = board.model.as_ref().unwrap();
    assert_eq!(
        (summary.calls, summary.batches_done, summary.skipped),
        (calls, summary.batches_total, false)
    );
    assert!(summary.usd > 0.0 && summary.error.is_none());
    assert!(built.items.iter().all(|item| !item.pending));

    // Nothing planted reaches the request (what the temp file holds) or the board.
    let sent = runner.requests.lock().unwrap().join("\n");
    let shown = serde_json::to_string(&board).unwrap();
    for secret in secrets() {
        assert!(!sent.contains(&secret), "sent: {secret}");
        assert!(!shown.contains(&secret), "shown: {secret}");
    }
    let request: Value = serde_json::from_str(&runner.requests.lock().unwrap()[0]).unwrap();
    assert!(request.get("toolConfig").is_none());
    let max_tokens = request["inferenceConfig"]["maxTokens"].as_u64().unwrap();
    assert!(max_tokens % model::ITEM_MAX_TOKENS == 0 && max_tokens <= model::BATCH_MAX_TOKENS);
    assert_eq!(request["inferenceConfig"]["temperature"], 0);

    // Unchanged evidence: answered from the cache, no second call.
    let again = run_pass(&service).await;
    assert_eq!(runner.calls.load(Ordering::SeqCst), calls);
    let summary = again.model.as_ref().unwrap();
    assert!(summary.skipped);
    assert!(summary.cached_items >= 3);
    let old = again
        .built
        .as_ref()
        .unwrap()
        .items
        .iter()
        .find(|item| item.id == "a:a-old")
        .unwrap();
    assert_eq!(old.section, Some(Section::LikelyDone));
}

async fn assert_falls_back(
    reply: impl Fn(&[String]) -> Result<Value, ModelError> + Send + Sync + 'static,
    expected: &str,
) {
    let runner = FakeRunner::new(reply);
    let (service, _temp) = make_service(WorkBoardSettings::with_model(
        "claude-haiku-4-5",
        runner.clone(),
    ))
    .await;
    let board = run_pass(&service).await;
    let summary = board.model.as_ref().unwrap();
    let error = summary.error.as_deref().unwrap_or_default();
    assert!(error.contains(expected), "{error}");
    let built = board.built.as_ref().unwrap();
    // Deterministic tiebreak, no AI notes, nothing pending.
    let old = built
        .items
        .iter()
        .find(|item| item.id == "a:a-old")
        .unwrap();
    assert_eq!(old.section, Some(Section::LikelyDone));
    assert!(
        built
            .items
            .iter()
            .all(|item| item.ai.is_none() && !item.pending)
    );
}

#[tokio::test]
async fn bad_json_max_tokens_and_runner_errors_fall_back() {
    assert_falls_back(
        |_| Ok(converse_reply("not json at all", "end_turn")),
        "not valid JSON",
    )
    .await;
    assert_falls_back(
        |ids| Ok(converse_reply(&verdicts_for(ids), "max_tokens")),
        "maxTokens",
    )
    .await;
    assert_falls_back(|_| Err(ModelError::Timeout(10)), "timed out").await;
    assert_falls_back(
        |_| {
            Err(ModelError::Failed {
                code: "AccessDenied".to_owned(),
                message: "no".to_owned(),
            })
        },
        "AccessDenied",
    )
    .await;
}

#[tokio::test]
async fn cost_caps_refuse_before_any_call() {
    // An unknown model id is priced at the highest rate; with the daily cap
    // nearly spent, not even one worst-case item fits.
    let runner = FakeRunner::new(|_| unreachable!("no call may be made"));
    let mut settings = WorkBoardSettings::with_model("mystery-model", runner.clone());
    settings.spent_today_usd = 0.97;
    let (service, _temp) = make_service(settings).await;
    match service.start(true, None) {
        Err(StartError::CostCap(message)) => assert!(message.contains("daily cap"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    // The model pass itself re-checks with the real request size (here the
    // daily cap is nearly spent).
    let runner = FakeRunner::new(|_| unreachable!("no call may be made"));
    let (service, _temp) = make_service(WorkBoardSettings::with_model(
        "claude-haiku-4-5",
        runner.clone(),
    ))
    .await;
    service.inner.budget.add(0.999);
    let board = run_pass(&service).await;
    assert!(board.model.unwrap().error.unwrap().contains("daily cap"));
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    // A disabled model is a 400, not a cap.
    let (off, _temp) = make_service(WorkBoardSettings::disabled()).await;
    assert_eq!(off.start(true, None), Err(StartError::ModelDisabled));
}

#[test]
fn budget_caps() {
    let budget = Budget::default();
    assert!(budget.check(1_000, 800, 0.01).is_ok());
    assert_eq!(
        budget.check(40_001, 800, 0.01),
        Err(CapError::InputTokens(40_001))
    );
    assert_eq!(
        budget.check(1_000, 4_001, 0.01),
        Err(CapError::OutputTokens(4_001))
    );
    assert_eq!(
        budget.check(1_000, 800, 0.11),
        Err(CapError::PerGenerate(0.11))
    );
    budget.add(0.97);
    assert_eq!(budget.check(1_000, 800, 0.05), Err(CapError::Daily(0.05)));
    let (input, output, usd) = model::estimate("claude-haiku-4-5", 40_000, 2, 7);
    assert!(input >= 10_000 && output == 1_400 && usd > 0.0);
    assert!(model::worst_case("claude-haiku-4-5", 32) <= model::MAX_GENERATE_USD);
}

#[test]
fn validation_drops_unknown_ids_uncited_actions_and_bad_confidence() {
    let ids = ["a", "b"]
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<HashSet<_>>();
    let long = "x".repeat(400);
    let reply = json!([
        {"id": "a", "verdict": "finished", "confidence": 0.9, "one_line": long, "next_actions": [
            {"text": "one", "source_id": "a"}, {"text": "two", "source_id": "b"},
            {"text": "three", "source_id": "a"}, {"text": "four", "source_id": "a"},
            {"text": "cited elsewhere", "source_id": "zzz"}], "evidence": "weird", "quote": long},
        {"id": "b", "verdict": "needs_work", "confidence": 1.5},
        {"id": "c", "verdict": "finished", "confidence": 0.9}
    ])
    .to_string();
    let verdicts = model::validate(&reply, &ids).unwrap();
    assert_eq!(verdicts.len(), 1);
    let a = &verdicts["a"];
    assert_eq!(a.next_actions, ["one", "two", "three"]);
    assert!(a.one_line.chars().count() <= 100);
    assert!(a.quote.chars().count() <= 120);
    assert_eq!(a.evidence, "objective");
    assert!(model::validate(r#"[{"id":"a","verdict":"maybe","confidence":0.5}]"#, &ids).is_err());
    assert!(
        model::validate(
            r#"[{"id":"a","verdict":"finished","confidence":0.5,"extra":1}]"#,
            &ids
        )
        .is_err()
    );
}

#[test]
fn tail_scrub_drops_env_auth_and_pem_and_redacts_paths() {
    let s = secrets();
    let tail = format!(
        "$ export AWS_SECRET_ACCESS_KEY={}\nAPI_TOKEN=abc\ncurl -H 'Authorization: Bearer {}' x\n{}\n{}\n{}\nedited {}/main.rs\nmysql -p{} db\nall tests passed",
        s[0],
        s[1],
        pem_marker("BEGIN"),
        s[4],
        pem_marker("END"),
        s[3],
        s[2]
    );
    let scrubbed = model::scrub_tail(&tail, model::MAX_TAIL_BYTES);
    for secret in &s {
        assert!(
            !scrubbed.contains(secret.as_str()),
            "{secret} in {scrubbed}"
        );
    }
    assert!(scrubbed.contains("all tests passed"));
    assert!(scrubbed.contains("[path]"));
    let long = (0..200)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let kept = model::scrub_tail(&long, 64);
    assert!(kept.len() <= 64 && kept.ends_with("line 199"));
    assert!(
        model::scrub_tail(&long, model::MAX_TAIL_BYTES)
            .lines()
            .count()
            <= 60
    );
}

#[test]
fn fingerprints_change_with_evidence_and_the_lru_is_bounded() {
    let inputs = fixture();
    let built = board::build(&inputs, None);
    let item = built
        .items
        .iter()
        .find(|item| item.id == "a:a-old")
        .unwrap();
    let facts = facts_key(&inputs, item);
    let first: Evidence = evidence(&inputs, item, Some("tail one"));
    let same = evidence(&inputs, item, Some("tail one"));
    let changed = evidence(&inputs, item, Some("tail two"));
    assert_eq!(
        fingerprint("m", &facts, &first),
        fingerprint("m", &facts, &same)
    );
    assert_ne!(
        fingerprint("m", &facts, &first),
        fingerprint("m", &facts, &changed)
    );
    assert_ne!(
        fingerprint("m", &facts, &first),
        fingerprint("other", &facts, &first)
    );
    // Same evidence, changed facts (e.g. the runtime moved): a new key.
    let mut moved = item.clone();
    moved.mark.state_change_sequence += 1;
    assert_ne!(
        fingerprint("m", &facts, &first),
        fingerprint("m", &facts_key(&inputs, &moved), &first)
    );
    // Evidence carries the status and age; the tail size is fixed per item.
    assert_eq!(first.status, item.status_line);
    assert_eq!(first.age_days, item.age_ms / (24 * 60 * 60 * 1000));
    let long = "x".repeat(20_000);
    let bounded = evidence(&inputs, item, Some(&long));
    assert!(bounded.tail.len() <= super::pass::TAIL_BYTES_PER_ITEM);
    let verdict = model::ItemVerdict {
        verdict: Verdict::NeedsWork,
        confidence: 0.7,
        one_line: String::new(),
        next_actions: Vec::new(),
        evidence: "tail".to_owned(),
        quote: String::new(),
        title: String::new(),
        about: String::new(),
        status: String::new(),
        your_move: String::new(),
    };
    let mut lru = Lru::default();
    for index in 0..300 {
        lru.put(format!("k{index}"), verdict.clone());
    }
    assert_eq!(lru.len(), 256);
    assert!(lru.get("k0").is_none());
    assert!(lru.get("k299").is_some());
}

#[test]
fn the_pass_trims_to_the_largest_prefix_under_the_caps() {
    let item = |index: usize| Evidence {
        id: format!("a:{index}"),
        title: "stale item".to_owned(),
        objective: "objective".to_owned(),
        status: "Idle".to_owned(),
        age_days: 9,
        report: None,
        receipt_blockers: Vec::new(),
        snapshot_next: Vec::new(),
        tail: "x".repeat(super::pass::TAIL_BYTES_PER_ITEM),
        place: "Gone quiet".to_owned(),
        needs_title: false,
        digest: String::new(),
    };
    let many = (0..model::MAX_MODEL_ITEMS).map(item).collect::<Vec<_>>();
    let budget = Budget::default();
    // An unknown id at the highest rate: all 32 do not fit $0.10.
    let (count, batches, estimates) =
        super::pass::affordable(&budget, "mystery-model", 0, &many).unwrap();
    assert!(count >= 1 && count < many.len(), "{count}");
    assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), count);
    assert_eq!(
        batches[0][0].id, "a:0",
        "the stalest (first) items are kept"
    );
    let usd = estimates.iter().map(|estimate| estimate.2).sum::<f64>();
    assert!(usd <= model::MAX_GENERATE_USD);
    // Nothing fits once the daily cap is spent.
    budget.add(model::DAILY_USD);
    assert!(matches!(
        super::pass::affordable(&budget, "mystery-model", 0, &many),
        Err(CapError::Daily(_))
    ));
}

/// A stand-in `aws` whose profile resolution (the `credential_process`)
/// is slow and serialized, like the live `credential-process`: every call
/// that names `--profile` waits for a lock and sleeps. It logs which calls
/// ran and whether they carried environment credentials (never the values).
fn slow_credential_aws(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("aws");
    let body = format!(
        r#"#!/bin/sh
log="{dir}/calls.log"
case " $* " in
  *" --profile "*)
    while ! mkdir "{dir}/cp.lock" 2>/dev/null; do sleep 0.05; done
    sleep 1.5
    echo "credential_process $1" >> "$log"
    rmdir "{dir}/cp.lock"
    ;;
esac
if [ -n "$AWS_ACCESS_KEY_ID" ]; then creds=env; else creds=profile; fi
case "$1" in
  configure)
    echo "export" >> "$log"
    printf '%s' '{{"Version":1,"AccessKeyId":"FAKEACCESSKEYID","SecretAccessKey":"fake-secret-value","SessionToken":"fake-session-token","Expiration":"2099-01-01T00:00:00Z"}}'
    ;;
  sts) echo "sts $creds" >> "$log"; echo "123456789012" ;;
  bedrock-runtime)
    sleep 0.3
    echo "converse $creds" >> "$log"
    printf '%s' '{{"stopReason":"end_turn","usage":{{"inputTokens":1000,"outputTokens":100}},"output":{{"message":{{"content":[{{"text":"[]"}}]}}}}}}'
    ;;
  *) exit 3 ;;
esac
"#,
        dir = dir.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

#[tokio::test]
async fn a_slow_credential_process_runs_once_and_every_batch_finishes() {
    use super::tests::board_tests::{assignment, candidate, worker};
    use yard_domain::{AssignmentLifecycle, ObservedStatus, WorkerAvailability};
    let aws_dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(crate::model::AwsConverse::new(
        slow_credential_aws(aws_dir.path()),
        "board".to_owned(),
        "us-west-2".to_owned(),
        "123456789012".to_owned(),
        aws_dir.path().to_path_buf(),
        std::time::Duration::from_secs(10),
    ));
    let (service, _temp) =
        make_service(WorkBoardSettings::with_model("claude-haiku-4-5", runner)).await;
    // 28 more stale, ambiguous assignments: the full four batches of at
    // most eight, all in flight at once.
    let mut inputs = fixture();
    let now = inputs.now_ms;
    for index in 0..28 {
        let id = format!("w-stale-{index}");
        let stale = worker(
            &id,
            Some(&id),
            Some(ObservedStatus::Idle),
            now - 3 * 24 * 60 * 60 * 1000,
        );
        inputs.assignments.push(assignment(
            &format!("a-stale-{index}"),
            "alpha",
            stale.clone(),
            AssignmentLifecycle::Active,
            now - 3 * 24 * 60 * 60 * 1000,
        ));
        inputs.workers.push(candidate(
            stale,
            WorkerAvailability::Assigned,
            Some("alpha"),
        ));
    }
    let built = board::build(&inputs, None);
    service.publish(|board| board.generation_id = Some("g".to_owned()));
    let clock = std::time::Instant::now();
    service
        .model_pass("g", &mut inputs, None, built, &[], now)
        .await;
    let elapsed = clock.elapsed();
    let board = service.board();
    let summary = board.model.as_ref().unwrap();
    eprintln!(
        "model pass: {} ms, batches {}/{}, calls {}",
        elapsed.as_millis(),
        summary.batches_done,
        summary.batches_total,
        summary.calls
    );
    assert_eq!(summary.batches_total, model::MAX_BATCHES, "{summary:?}");
    assert_eq!(summary.batches_done, model::MAX_BATCHES, "{summary:?}");
    assert!(summary.error.is_none(), "{summary:?}");
    // One export (one credential_process run) instead of one per spawn.
    let log = std::fs::read_to_string(aws_dir.path().join("calls.log")).unwrap();
    let count = |line: &str| log.lines().filter(|entry| *entry == line).count();
    assert_eq!(count("credential_process configure"), 1, "{log}");
    assert_eq!(count("export"), 1, "{log}");
    assert_eq!(count("sts env"), 1, "{log}");
    assert_eq!(count("converse env"), model::MAX_BATCHES, "{log}");
    assert_eq!(
        log.lines()
            .filter(|entry| entry.contains("profile"))
            .count(),
        0,
        "{log}"
    );
    // No wall-clock bound (flaky on a loaded desk): the call counts above
    // (one export, no per-call profile resolution, every batch done) are
    // what proves the fix; the time is printed only.
    // The exported values are never shown.
    let shown = serde_json::to_string(&board).unwrap();
    for secret in ["FAKEACCESSKEYID", "fake-secret-value", "fake-session-token"] {
        assert!(!shown.contains(secret) && !log.contains(secret));
    }
}
