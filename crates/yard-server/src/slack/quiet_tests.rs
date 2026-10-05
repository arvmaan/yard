//! Quiet hours end to end: detector → policy → digest → mock Slack, with an
//! injected clock. Nothing here reaches slack.com or AWS.

use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tempfile::TempDir;
use yard_store::YardStore;

use super::{
    NotifierParts, SlackNotifier,
    client::SlackApi,
    policy::{
        QuietParts,
        tests::{NEXT_MORNING, TestClock, WORKDAY_EVENING, WORKDAY_MORNING, parts},
    },
    quiet::{InstantKinds, QuietConfig},
    tests::{
        MockSlack, TOKEN, capture_logs, fake_aws, instant_timing, set_orchestrator_status,
        settings, store_with_project,
    },
};
use crate::config::SlackConfig;

fn quiet_notifier(
    temp: &TempDir,
    store: Arc<dyn YardStore>,
    slack: &MockSlack,
    quiet: QuietParts,
) -> SlackNotifier {
    capture_logs();
    SlackNotifier::with_parts(
        SlackConfig::Enabled(settings(None)),
        store,
        NotifierParts {
            api: SlackApi::with_base_url(slack.base_url.clone()),
            aws_binary: fake_aws(temp.path(), TOKEN).into(),
            thread_file: Some(temp.path().join("slack-threads.json")),
            timing: instant_timing(),
            min_interval: Duration::ZERO,
            inbound: super::hub::InboundParts::default(),
            quiet,
        },
    )
}

fn posts(slack: &MockSlack) -> Vec<Value> {
    slack
        .calls("chat.postMessage")
        .into_iter()
        .map(|call| call.body)
        .collect()
}

async fn block_orchestrator(notifier: &SlackNotifier, temp: &TempDir, sequence: u64) {
    set_orchestrator_status(temp, "blocked", sequence);
    notifier.tick().await; // pending
    notifier.tick().await; // settled
}

#[tokio::test]
async fn evening_events_wait_for_one_morning_digest_that_survives_a_restart() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let clock = TestClock::at(WORKDAY_EVENING);
    let notifier = quiet_notifier(&temp, store.clone(), &slack, parts(&clock, Some(&temp)));
    notifier.tick().await; // baseline
    block_orchestrator(&notifier, &temp, 2).await;
    notifier.tick().await;
    assert!(posts(&slack).is_empty(), "{:?}", posts(&slack));
    let quiet = notifier.status().quiet.unwrap();
    assert!(quiet.active);
    assert_eq!(quiet.held_count, 1);
    assert_eq!(quiet.until, quiet.next_delivery_at);
    assert!(temp.path().join("slack-held.json").exists());
    drop(notifier);

    // Restart overnight; the worker is still blocked in the morning.
    let notifier = quiet_notifier(&temp, store, &slack, parts(&clock, Some(&temp)));
    notifier.tick().await;
    assert_eq!(notifier.status().quiet.unwrap().held_count, 1);
    assert!(posts(&slack).is_empty());
    clock.set(NEXT_MORNING);
    notifier.tick().await;
    let sent = posts(&slack);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let text = sent[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("While you were away: 1 waiting for input"),
        "{text}"
    );
    let blocks = sent[0]["blocks"].to_string();
    assert!(blocks.contains(":sunny: While you were away"), "{blocks}");
    assert!(blocks.contains("Checkout &lt;!channel&gt;"), "{blocks}");
    // Inbound is off: no Answer button, only the Yard link.
    assert!(!blocks.contains("Answer"), "{blocks}");
    // Posted once.
    notifier.tick().await;
    notifier.tick().await;
    assert_eq!(posts(&slack).len(), 1);
    let quiet = notifier.status().quiet.unwrap();
    assert!(!quiet.active);
    assert_eq!(quiet.held_count, 0);
    assert_eq!(quiet.next_delivery_at, None);
}

#[tokio::test]
async fn resolved_items_are_dropped_and_an_empty_digest_is_not_sent() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let clock = TestClock::at(WORKDAY_EVENING);
    let notifier = quiet_notifier(&temp, store, &slack, parts(&clock, Some(&temp)));
    notifier.tick().await;
    block_orchestrator(&notifier, &temp, 2).await;
    assert_eq!(notifier.status().quiet.unwrap().held_count, 1);
    // Answered in Yard overnight.
    set_orchestrator_status(&temp, "working", 3);
    notifier.tick().await;
    clock.set(NEXT_MORNING);
    notifier.tick().await;
    notifier.tick().await;
    assert!(posts(&slack).is_empty(), "{:?}", posts(&slack));
    assert_eq!(notifier.status().quiet.unwrap().held_count, 0);
}

#[tokio::test]
async fn in_hours_blocked_is_instant_and_other_kinds_wait_for_the_digest() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let clock = TestClock::at(WORKDAY_MORNING);
    let notifier = quiet_notifier(&temp, store.clone(), &slack, parts(&clock, None));
    notifier.tick().await;
    block_orchestrator(&notifier, &temp, 2).await;
    assert_eq!(posts(&slack).len(), 1);
    assert!(
        posts(&slack)[0]["text"]
            .as_str()
            .unwrap()
            .contains("blocked")
    );

    // With nothing instant, a blocked agent waits for the two-hour digest.
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let mut quiet = parts(&clock, None);
    quiet.config = QuietConfig {
        instant: InstantKinds {
            blocked: false,
            failed: false,
            review: false,
        },
        ..QuietConfig::default()
    };
    let notifier = quiet_notifier(&temp, store, &slack, quiet);
    notifier.tick().await;
    block_orchestrator(&notifier, &temp, 2).await;
    assert!(posts(&slack).is_empty());
    clock.advance(Duration::from_secs(119 * 60));
    notifier.tick().await;
    assert!(posts(&slack).is_empty());
    clock.advance(Duration::from_secs(60));
    notifier.tick().await;
    let sent = posts(&slack);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Yard digest: 1 waiting for input")
    );
}

#[tokio::test]
async fn the_status_endpoint_reports_quiet_state() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let clock = TestClock::at(WORKDAY_EVENING);
    let notifier = quiet_notifier(&temp, store, &slack, parts(&clock, None));
    let json = serde_json::to_value(notifier.status()).unwrap();
    let quiet = &json["quiet"];
    assert_eq!(quiet["active"], true);
    assert_eq!(quiet["held_count"], 0);
    assert_eq!(quiet["muted_until"], Value::Null);
    assert_eq!(quiet["next_delivery_at"], Value::Null);
    let morning = chrono::DateTime::parse_from_rfc3339(NEXT_MORNING)
        .unwrap()
        .timestamp_millis();
    assert_eq!(quiet["until"], morning);
    // Off has no quiet block (the HTTP contract for "off" is unchanged).
    let off = serde_json::to_value(SlackNotifier::off().status()).unwrap();
    assert!(off.get("quiet").is_none());
}
