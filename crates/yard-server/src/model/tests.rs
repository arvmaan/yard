use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use serde_json::json;

use super::{AwsConverse, ConverseRunner, ModelError, Stop, parse, price, request};

/// A stand-in `aws` binary: answers `sts` with `account`, and for
/// `converse` copies the request file (and its mode) next to itself before
/// printing a canned reply.
fn fake_aws(dir: &Path, account: &str, reply: &str) -> std::path::PathBuf {
    let script = dir.join("aws");
    let body = format!(
        r#"#!/bin/sh
case "$1" in
  sts) echo "{account}" ;;
  bedrock-runtime)
    file="${{4#file://}}"
    cp "$file" "{dir}/captured.json"
    stat -f %Lp "$file" > "{dir}/captured.mode" 2>/dev/null || stat -c %a "$file" > "{dir}/captured.mode"
    echo "$@" > "{dir}/captured.argv"
    cat <<'JSON'
{reply}
JSON
    ;;
  *) echo "An error occurred (Unknown) when calling the X operation: nope" >&2; exit 3 ;;
esac
"#,
        dir = dir.display()
    );
    fs::write(&script, body).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn runner(dir: &Path, account: &str, reply: &str) -> AwsConverse {
    AwsConverse::new(
        fake_aws(dir, account, reply),
        "board".to_owned(),
        "us-west-2".to_owned(),
        "123456789012".to_owned(),
        dir.to_path_buf(),
        Duration::from_secs(10),
    )
}

const REPLY: &str = r#"{"stopReason":"end_turn","usage":{"inputTokens":1000,"outputTokens":100},"output":{"message":{"content":[{"text":"[]"}]}}}"#;

#[tokio::test]
async fn converse_uses_a_private_temp_file_that_is_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let aws = runner(temp.path(), "123456789012", REPLY);
    let body = request("claude-haiku-4-5", "system", "user text", 800);
    let response = aws.converse(&body).await.unwrap();
    assert_eq!(response["stopReason"], "end_turn");
    let captured: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("captured.json")).unwrap())
            .unwrap();
    assert_eq!(captured, body);
    assert_eq!(
        fs::read_to_string(temp.path().join("captured.mode"))
            .unwrap()
            .trim(),
        "600"
    );
    let argv = fs::read_to_string(temp.path().join("captured.argv")).unwrap();
    assert!(argv.contains("--profile board"), "{argv}");
    assert!(argv.contains("--region us-west-2"));
    // The request file is gone after the call.
    let leftovers = fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("yard-board-")
        })
        .count();
    assert_eq!(leftovers, 0);
}

#[tokio::test]
async fn converse_refuses_tools_and_a_foreign_account() {
    let temp = tempfile::tempdir().unwrap();
    let aws = runner(temp.path(), "123456789012", REPLY);
    let mut with_tools = request("m", "s", "u", 10);
    with_tools["toolConfig"] = json!({"tools": []});
    assert!(matches!(
        aws.converse(&with_tools).await,
        Err(ModelError::Invalid(_))
    ));

    let other = tempfile::tempdir().unwrap();
    let foreign = runner(other.path(), "999999999999", REPLY);
    assert_eq!(
        foreign.converse(&request("m", "s", "u", 10)).await,
        Err(ModelError::AccountMismatch)
    );
    assert!(!other.path().join("captured.json").exists());
}

#[test]
fn parse_reads_text_usage_and_stop() {
    let reply = parse(
        "claude-haiku-4-5",
        &serde_json::from_str(r#"{"stopReason":"max_tokens","usage":{"inputTokens":10,"outputTokens":5},"output":{"message":{"content":[{"text":"a"},{"text":"b"}]}}}"#).unwrap(),
    )
    .unwrap();
    assert_eq!(reply.text, "ab");
    assert_eq!(reply.stop, Stop::MaxTokens);
    assert!(parse("m", &json!({"usage": {}})).is_err());
}

#[test]
fn unknown_models_are_priced_at_the_highest_rate() {
    let known = price::cost_usd("claude-haiku-4-5", 1_000_000, 0);
    let unknown = price::cost_usd("mystery-model", 1_000_000, 0);
    assert!(unknown > known);
    assert!((unknown - price::UNKNOWN_MODEL.input).abs() < 1e-9);
}

#[test]
fn an_unknown_model_is_never_priced_below_any_known_row() {
    for row in price::PRICES {
        let rates = std::iter::once(row.rate).chain(row.long_context.map(|(_, rate)| rate));
        for rate in rates {
            assert!(price::UNKNOWN_MODEL.input >= rate.input, "{}", row.stem);
            assert!(price::UNKNOWN_MODEL.output >= rate.output, "{}", row.stem);
        }
    }
    let unknown = price::cost_usd("claude-opus-6", 10_000, 1_000);
    let opus4 = price::cost_usd("claude-opus-4", 10_000, 1_000);
    assert!(unknown >= opus4);
}
