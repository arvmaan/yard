//! Provider session identity taken from a pane's foreground command line.
//!
//! Herdr only knows a provider session id for agents it launched itself or
//! that report through its integration hooks. When an agent is relaunched by
//! hand (`codex resume <id>`, `claude --resume <id>`), the pane's foreground
//! process is the ground truth for which conversation runs there. This module
//! holds the pure, anchored parser for those command lines and the precedence
//! rule that merges a command-line id into an observed inventory.

use crate::{ProviderSessionRef, RuntimeInventory};

/// Providers whose resume command lines Yard recognizes.
pub const COMMAND_LINE_SESSION_PROVIDERS: [&str; 2] = ["codex", "claude"];

/// One process in a pane's foreground process group, as reported by the
/// runtime (Herdr `pane.process_info` `foreground_processes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundProcess {
    pub pid: u32,
    pub name: String,
    pub argv: Option<Vec<String>>,
}

/// The foreground job of one pane at the time it was inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneForegroundJob {
    pub pane_id: String,
    /// `None` when the runtime could not determine a foreground process
    /// group; the job then carries no process identity.
    pub process_group_id: Option<u32>,
    pub processes: Vec<ForegroundProcess>,
}

/// A provider session id derived from one pane's foreground command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLineProviderSession {
    pub terminal_id: String,
    pub pane_id: String,
    pub provider: String,
    pub value: String,
}

/// One observation whose provider session was set from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLineSessionOverride {
    pub terminal_id: String,
    pub pane_id: String,
    /// What Herdr reported for the pane; `None` when it reported nothing.
    pub reported: Option<ProviderSessionRef>,
    pub derived: ProviderSessionRef,
}

/// Return `true` for the canonical lowercase hyphenated UUID form
/// (`8-4-4-4-12` hex digits). Other spellings (uppercase, braces, URNs,
/// simple form) and the nil UUID are rejected so a match is exact.
#[must_use]
pub fn is_canonical_session_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let shaped = bytes.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
    });
    shaped && bytes.iter().any(|byte| !matches!(byte, b'0' | b'-'))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Parse a provider session id from one process argv.
///
/// Accepted, anchored forms (`argv[0]`'s basename must equal `provider`):
///
/// * Codex: `codex [FLAG]... resume [FLAG]... <uuid> [ARG]...`. Every token
///   before `resume` and between `resume` and the id must be a flag (start
///   with `-`); the first positional after `resume` must be the id. A flag
///   that takes a separate value (`-m gpt-5`) therefore makes the line
///   unparseable, which is deliberate: only unambiguous lines count.
/// * Claude: `claude [ARG]... (--resume <uuid> | -r <uuid> | --resume=<uuid>) [ARG]...`.
///   A line that names more than one resume id, or that also passes
///   `--fork-session` (which starts a new session), yields `None`.
///
/// Anything after `--` is not inspected for Claude, and the id must be a
/// canonical UUID (see [`is_canonical_session_uuid`]).
#[must_use]
pub fn provider_session_from_argv(provider: &str, argv: &[String]) -> Option<String> {
    let (program, arguments) = argv.split_first()?;
    if basename(program) != provider {
        return None;
    }
    match provider {
        "codex" => codex_resume_id(arguments),
        "claude" => claude_resume_id(arguments),
        _ => None,
    }
}

fn codex_resume_id(arguments: &[String]) -> Option<String> {
    let mut tokens = arguments.iter().map(String::as_str);
    // Leading global flags, then the literal `resume` subcommand.
    loop {
        let token = tokens.next()?;
        if token == "resume" {
            break;
        }
        if !token.starts_with('-') || token == "--" {
            return None;
        }
    }
    // Flags, then the first positional must be the session id.
    let mut after_separator = false;
    for token in tokens {
        if !after_separator && token == "--" {
            after_separator = true;
            continue;
        }
        if !after_separator && token.starts_with('-') {
            continue;
        }
        return is_canonical_session_uuid(token).then(|| token.to_owned());
    }
    None
}

fn claude_resume_id(arguments: &[String]) -> Option<String> {
    let mut found: Option<&str> = None;
    let mut tokens = arguments.iter().map(String::as_str);
    while let Some(token) = tokens.next() {
        let candidate = match token {
            "--" => break,
            "--fork-session" => return None,
            "--resume" | "-r" => tokens.next()?,
            _ => match token.strip_prefix("--resume=") {
                Some(value) => value,
                None => continue,
            },
        };
        if !is_canonical_session_uuid(candidate) {
            return None;
        }
        match found {
            Some(previous) if previous != candidate => return None,
            _ => found = Some(candidate),
        }
    }
    found.map(str::to_owned)
}

/// Derive the provider session id running in a pane's foreground job.
///
/// Only members of the pane's foreground process group count (a background
/// job or a daemon in another group never does). Every member whose argv
/// parses must agree; two members naming different ids yield `None`.
/// Members whose `argv[0]` basename is not `provider`, or whose line does
/// not parse (for example `codex app-server` or a Node wrapper around the
/// native binary), are ignored.
#[must_use]
pub fn provider_session_from_foreground(provider: &str, job: &PaneForegroundJob) -> Option<String> {
    job.process_group_id?;
    let mut found: Option<String> = None;
    for process in &job.processes {
        let Some(value) = process
            .argv
            .as_deref()
            .and_then(|argv| provider_session_from_argv(provider, argv))
        else {
            continue;
        };
        match &found {
            Some(previous) if *previous != value => return None,
            _ => found = Some(value),
        }
    }
    found
}

/// The provider session reference Yard uses for a command-line id. It uses
/// the same `source`/`kind` shape as Herdr's own `herdr:<agent>` reports so
/// a binding recorded from either source compares equal.
#[must_use]
pub fn command_line_provider_session_ref(provider: &str, value: &str) -> ProviderSessionRef {
    ProviderSessionRef {
        source: format!("herdr:{provider}"),
        provider: provider.to_owned(),
        kind: "id".to_owned(),
        value: value.to_owned(),
    }
}

/// Merge command-line provider sessions into an observed inventory.
///
/// Precedence per agent (matched by terminal id, pane id and provider):
///
/// * Herdr reports no session: use the command-line id.
/// * Herdr reports the same id: keep Herdr's report unchanged.
/// * Herdr reports a different id: a process's argv never changes, so after
///   an in-process session switch (Claude `/clear` or `/resume`, Codex
///   `/new`) the command line names the *previous* conversation while
///   Herdr's report is current. The command line therefore overrides a
///   disagreeing report only with evidence that the report is misattributed
///   and none that the command line is stale:
///   * stale: the same command-line id is also derived on another terminal
///     (that pane really runs it) → keep Herdr's report;
///   * misattributed: the provider is Codex (its hooks run in one shared
///     `codex app-server` daemon and report for the wrong pane), or Herdr's
///     value is also reported or derived on another terminal → use the
///     command line;
///   * otherwise (for example Claude, whose hook runs per process) keep
///     Herdr's report.
///
/// Agents without a derived id keep their Herdr report. The matching pane
/// observation (same terminal id) receives the same reference so worker and
/// pane views agree. Returns every observation that changed.
pub fn apply_command_line_provider_sessions(
    inventory: &mut RuntimeInventory,
    derived: &[CommandLineProviderSession],
) -> Vec<CommandLineSessionOverride> {
    let reports_by_terminal: Vec<(String, String)> = inventory
        .workers
        .iter()
        .filter_map(|worker| {
            worker
                .provider_session
                .as_ref()
                .map(|reported| (worker.terminal_id.clone(), reported.value.clone()))
        })
        .collect();
    let mut overrides = Vec::new();
    for session in derived {
        let Some(worker) = inventory.workers.iter_mut().find(|worker| {
            worker.terminal_id == session.terminal_id
                && worker.pane_id == session.pane_id
                && worker.provider.as_deref() == Some(session.provider.as_str())
        }) else {
            continue;
        };
        if let Some(reported) = worker.provider_session.as_ref() {
            if reported.value == session.value
                || !command_line_overrides_report(
                    session,
                    &reported.value,
                    derived,
                    &reports_by_terminal,
                )
            {
                continue;
            }
        }
        let derived_ref = command_line_provider_session_ref(&session.provider, &session.value);
        let reported = worker.provider_session.replace(derived_ref.clone());
        for pane in inventory
            .panes
            .iter_mut()
            .filter(|pane| pane.terminal_id == session.terminal_id)
        {
            pane.provider_session = Some(derived_ref.clone());
        }
        overrides.push(CommandLineSessionOverride {
            terminal_id: session.terminal_id.clone(),
            pane_id: session.pane_id.clone(),
            reported,
            derived: derived_ref,
        });
    }
    overrides
}

/// Whether `session` (derived from the command line) should replace Herdr's
/// disagreeing `reported` value; see [`apply_command_line_provider_sessions`].
fn command_line_overrides_report(
    session: &CommandLineProviderSession,
    reported: &str,
    derived: &[CommandLineProviderSession],
    reports_by_terminal: &[(String, String)],
) -> bool {
    let elsewhere = |value: &str| {
        derived
            .iter()
            .any(|other| other.terminal_id != session.terminal_id && other.value == value)
    };
    if elsewhere(&session.value) {
        return false;
    }
    session.provider == "codex"
        || elsewhere(reported)
        || reports_by_terminal
            .iter()
            .any(|(terminal_id, value)| *terminal_id != session.terminal_id && value == reported)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{FocusObservation, ObservedStatus, ObservedWorker, PaneObservation};

    const ID: &str = "01a0b10f-4a75-7841-8e1d-aa6f12919c45";
    const OTHER: &str = "01a0eb18-71ea-7c2b-9d3e-0123456789ab";

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    fn parse(provider: &str, line: &str) -> Option<String> {
        provider_session_from_argv(provider, &argv(line))
    }

    #[test]
    fn canonical_uuid_syntax_is_exact() {
        assert!(is_canonical_session_uuid(ID));
        assert!(!is_canonical_session_uuid(&ID.to_uppercase()));
        assert!(!is_canonical_session_uuid(&ID.replace('-', "")));
        assert!(!is_canonical_session_uuid(&format!("{{{ID}}}")));
        assert!(!is_canonical_session_uuid(&format!("urn:uuid:{ID}")));
        assert!(!is_canonical_session_uuid(&ID[..35]));
        assert!(!is_canonical_session_uuid(&format!("{ID}0")));
        assert!(!is_canonical_session_uuid(
            "01a0b10f-4a75-7841-8e1d-aa6f12919cg5"
        ));
        assert!(!is_canonical_session_uuid(
            "01a0b10f4-a75-7841-8e1d-aa6f12919c45"
        ));
        assert!(!is_canonical_session_uuid(
            "00000000-0000-0000-0000-000000000000"
        ));
    }

    #[test]
    fn parses_codex_resume_forms() {
        assert_eq!(
            parse("codex", &format!("codex resume {ID}")),
            Some(ID.into())
        );
        assert_eq!(
            parse("codex", &format!("/opt/codex/bin/codex resume {ID}")),
            Some(ID.into())
        );
        assert_eq!(
            parse("codex", &format!("codex resume --yolo {ID}")),
            Some(ID.into())
        );
        assert_eq!(
            parse("codex", &format!("codex --yolo resume --model=gpt-5 {ID}")),
            Some(ID.into())
        );
        assert_eq!(
            parse("codex", &format!("codex resume -- {ID}")),
            Some(ID.into())
        );
        assert_eq!(
            parse("codex", &format!("codex resume {ID} continue --yolo")),
            Some(ID.into())
        );
    }

    #[test]
    fn rejects_unanchored_or_invalid_codex_lines() {
        for line in [
            "codex".to_owned(),
            "codex resume".to_owned(),
            "codex resume --last".to_owned(),
            "codex resume not-a-session".to_owned(),
            format!("codex resume {}", ID.to_uppercase()),
            format!("codex resume -m gpt-5 {ID}"),
            format!("codex -m gpt-5 resume {ID}"),
            format!("codex fork {ID}"),
            format!("codex exec resume {ID}"),
            format!("codex {ID}"),
            format!("codex -- resume {ID}"),
            format!("codex app-server {ID}"),
            format!("codex --resume {ID}"),
        ] {
            assert_eq!(parse("codex", &line), None, "{line}");
        }
    }

    #[test]
    fn requires_the_provider_basename_in_argv0() {
        for line in [
            format!("node /usr/lib/node_modules/@openai/codex/bin/codex resume {ID}"),
            format!("codex-x86_64-unknown-linux-musl resume {ID}"),
            format!("/usr/bin/codex.js resume {ID}"),
            format!("claude resume {ID}"),
            format!("sh -c codex resume {ID}"),
            // A `#!/bin/sh` launcher named `codex` (argv as the kernel reports it).
            format!("/bin/sh /tmp/script-bin/codex resume {ID}"),
            format!("xcodex resume {ID}"),
        ] {
            assert_eq!(parse("codex", &line), None, "{line}");
        }
        assert_eq!(parse("claude", &format!("codex --resume {ID}")), None);
        assert_eq!(parse("gemini", &format!("gemini --resume {ID}")), None);
        assert_eq!(provider_session_from_argv("codex", &[]), None);
    }

    #[test]
    fn parses_claude_resume_forms() {
        assert_eq!(
            parse("claude", &format!("claude --resume {ID}")),
            Some(ID.into())
        );
        assert_eq!(parse("claude", &format!("claude -r {ID}")), Some(ID.into()));
        assert_eq!(
            parse("claude", &format!("claude --resume={ID}")),
            Some(ID.into())
        );
        assert_eq!(
            parse(
                "claude",
                &format!(
                    "/home/u/.local/bin/claude --dangerously-skip-permissions --model sonnet -r {ID} --verbose"
                )
            ),
            Some(ID.into())
        );
        assert_eq!(
            parse("claude", &format!("claude --resume {ID} -r {ID}")),
            Some(ID.into())
        );
    }

    #[test]
    fn rejects_unanchored_or_invalid_claude_lines() {
        for line in [
            "claude".to_owned(),
            "claude --resume".to_owned(),
            "claude -c".to_owned(),
            format!("claude --resume --model {ID}"),
            format!("claude --resume my-session {ID}"),
            format!("claude --resume {ID} --fork-session"),
            format!("claude --fork-session -r {ID}"),
            format!("claude --resume {ID} -r {OTHER}"),
            format!("claude --session-id {ID}"),
            format!("claude -- --resume {ID}"),
            format!("claude {ID}"),
            format!("claude --resume={}", ID.to_uppercase()),
        ] {
            assert_eq!(parse("claude", &line), None, "{line}");
        }
    }

    fn process(pid: u32, line: &str) -> ForegroundProcess {
        ForegroundProcess {
            pid,
            name: line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned(),
            argv: Some(argv(line)),
        }
    }

    fn job(processes: Vec<ForegroundProcess>) -> PaneForegroundJob {
        PaneForegroundJob {
            pane_id: "w1:pB".to_owned(),
            process_group_id: Some(processes.first().map_or(1, |process| process.pid)),
            processes,
        }
    }

    #[test]
    fn foreground_job_yields_the_single_agreed_resume_id() {
        let wrapped = job(vec![
            process(10, &format!("node /usr/lib/codex/bin/codex.js resume {ID}")),
            process(
                11,
                &format!("/usr/lib/codex/vendor/codex/codex resume {ID}"),
            ),
            process(12, "codex app-server --listen stdio"),
        ]);
        assert_eq!(
            provider_session_from_foreground("codex", &wrapped),
            Some(ID.into())
        );

        let conflicting = job(vec![
            process(10, &format!("codex resume {ID}")),
            process(11, &format!("codex resume {OTHER}")),
        ]);
        assert_eq!(
            provider_session_from_foreground("codex", &conflicting),
            None
        );

        let wrong_provider = job(vec![process(10, &format!("codex resume {ID}"))]);
        assert_eq!(
            provider_session_from_foreground("claude", &wrong_provider),
            None
        );
    }

    #[test]
    fn only_the_foreground_process_group_counts() {
        let mut no_foreground = job(vec![process(10, &format!("codex resume {ID}"))]);
        no_foreground.process_group_id = None;
        assert_eq!(
            provider_session_from_foreground("codex", &no_foreground),
            None
        );

        let shell_only = job(vec![process(10, "-bash")]);
        assert_eq!(provider_session_from_foreground("codex", &shell_only), None);

        let unreadable = job(vec![ForegroundProcess {
            pid: 10,
            name: "codex".to_owned(),
            argv: None,
        }]);
        assert_eq!(provider_session_from_foreground("codex", &unreadable), None);
    }

    fn reported(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn observed(provider_session: Option<ProviderSessionRef>) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.9.1".to_owned(),
            protocol: 22,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: vec![PaneObservation {
                runtime_id: "w1:pB".to_owned(),
                pane_instance_id: None,
                terminal_id: "term-b".to_owned(),
                workspace_id: "w1".to_owned(),
                tab_id: "w1:t1".to_owned(),
                focused: false,
                cwd: None,
                foreground_cwd: None,
                label: None,
                provider: Some("codex".to_owned()),
                display_provider: None,
                status: ObservedStatus::Idle,
                tokens: BTreeMap::new(),
                provider_session: provider_session.clone(),
                revision: 1,
            }],
            workers: vec![ObservedWorker {
                runtime_id: "term-b".to_owned(),
                pane_instance_id: None,
                terminal_id: "term-b".to_owned(),
                workspace_id: "w1".to_owned(),
                tab_id: "w1:t1".to_owned(),
                pane_id: "w1:pB".to_owned(),
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
                provider_session,
                revision: 1,
            }],
            child_agents: Vec::new(),
        }
    }

    fn derived(value: &str) -> CommandLineProviderSession {
        CommandLineProviderSession {
            terminal_id: "term-b".to_owned(),
            pane_id: "w1:pB".to_owned(),
            provider: "codex".to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn command_line_fills_a_missing_herdr_session() {
        let mut inventory = observed(None);
        let overrides = apply_command_line_provider_sessions(&mut inventory, &[derived(ID)]);

        assert_eq!(inventory.workers[0].provider_session, Some(reported(ID)));
        assert_eq!(inventory.panes[0].provider_session, Some(reported(ID)));
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].reported, None);
        assert_eq!(overrides[0].derived, reported(ID));
    }

    #[test]
    fn command_line_wins_over_a_disagreeing_herdr_report() {
        let mut inventory = observed(Some(reported(OTHER)));
        let overrides = apply_command_line_provider_sessions(&mut inventory, &[derived(ID)]);

        assert_eq!(inventory.workers[0].provider_session, Some(reported(ID)));
        assert_eq!(inventory.panes[0].provider_session, Some(reported(ID)));
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].reported, Some(reported(OTHER)));
    }

    #[test]
    fn agreeing_or_absent_command_line_leaves_herdr_report_unchanged() {
        let mut custom = reported(ID);
        custom.source = "plugin:codex".to_owned();
        let mut agreeing = observed(Some(custom.clone()));
        assert!(apply_command_line_provider_sessions(&mut agreeing, &[derived(ID)]).is_empty());
        assert_eq!(agreeing.workers[0].provider_session, Some(custom));

        let mut neither = observed(None);
        assert!(apply_command_line_provider_sessions(&mut neither, &[]).is_empty());
        assert_eq!(neither.workers[0].provider_session, None);

        let mut herdr_only = observed(Some(reported(OTHER)));
        assert!(apply_command_line_provider_sessions(&mut herdr_only, &[]).is_empty());
        assert_eq!(
            herdr_only.workers[0].provider_session,
            Some(reported(OTHER))
        );
    }

    #[test]
    fn command_line_applies_only_to_the_same_pane_and_provider() {
        let mut moved = derived(ID);
        moved.pane_id = "w1:pC".to_owned();
        let mut other_kind = derived(ID);
        other_kind.provider = "claude".to_owned();
        let mut other_terminal = derived(ID);
        other_terminal.terminal_id = "term-c".to_owned();
        let mut inventory = observed(Some(reported(OTHER)));

        let overrides = apply_command_line_provider_sessions(
            &mut inventory,
            &[moved, other_kind, other_terminal],
        );

        assert!(overrides.is_empty());
        assert_eq!(inventory.workers[0].provider_session, Some(reported(OTHER)));
        assert_eq!(inventory.panes[0].provider_session, Some(reported(OTHER)));
    }

    /// Turn the single `observed` agent into `provider`, reporting `value`.
    fn as_provider(mut inventory: RuntimeInventory, provider: &str) -> RuntimeInventory {
        for session in inventory
            .workers
            .iter_mut()
            .filter_map(|worker| worker.provider_session.as_mut())
            .chain(
                inventory
                    .panes
                    .iter_mut()
                    .filter_map(|pane| pane.provider_session.as_mut()),
            )
        {
            session.source = format!("herdr:{provider}");
            session.provider = provider.to_owned();
        }
        inventory.workers[0].provider = Some(provider.to_owned());
        inventory.panes[0].provider = Some(provider.to_owned());
        inventory
    }

    /// Add a second agent on `term-c` / `w1:pC` with the same provider.
    fn with_second_agent(mut inventory: RuntimeInventory, value: Option<&str>) -> RuntimeInventory {
        let mut pane = inventory.panes[0].clone();
        let mut worker = inventory.workers[0].clone();
        let session = value.map(|value| {
            let mut session = reported(value);
            session.source = format!("herdr:{}", worker.provider.as_deref().unwrap_or("codex"));
            session.provider = worker.provider.clone().unwrap_or_default();
            session
        });
        pane.runtime_id = "w1:pC".to_owned();
        pane.terminal_id = "term-c".to_owned();
        pane.provider_session = session.clone();
        worker.runtime_id = "term-c".to_owned();
        worker.terminal_id = "term-c".to_owned();
        worker.pane_id = "w1:pC".to_owned();
        worker.provider_session = session;
        inventory.panes.push(pane);
        inventory.workers.push(worker);
        inventory
    }

    fn on_term_c(mut session: CommandLineProviderSession) -> CommandLineProviderSession {
        session.terminal_id = "term-c".to_owned();
        session.pane_id = "w1:pC".to_owned();
        session
    }

    fn claude(mut session: CommandLineProviderSession) -> CommandLineProviderSession {
        session.provider = "claude".to_owned();
        session
    }

    #[test]
    fn claude_in_process_switch_keeps_herdr_report_over_the_stale_command_line() {
        // `claude --resume ID` ran `/clear`: the per-process hook reports the
        // new conversation OTHER while argv still names ID.
        let mut inventory = as_provider(observed(Some(reported(OTHER))), "claude");
        let before = inventory.workers[0].provider_session.clone();

        let overrides =
            apply_command_line_provider_sessions(&mut inventory, &[claude(derived(ID))]);

        assert!(overrides.is_empty());
        assert_eq!(inventory.workers[0].provider_session, before);
        assert_eq!(inventory.panes[0].provider_session, before);
        assert_eq!(before.unwrap().value, OTHER);
    }

    #[test]
    fn claude_command_line_wins_when_herdr_value_belongs_to_another_terminal() {
        // Herdr reports OTHER on both panes; term-c's command line proves
        // OTHER runs there, so term-b's report is misattributed.
        let mut reported_twice = with_second_agent(
            as_provider(observed(Some(reported(OTHER))), "claude"),
            Some(OTHER),
        );
        let overrides =
            apply_command_line_provider_sessions(&mut reported_twice, &[claude(derived(ID))]);
        assert_eq!(overrides.len(), 1);
        assert_eq!(
            reported_twice.workers[0]
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some(ID)
        );

        let mut derived_elsewhere =
            with_second_agent(as_provider(observed(Some(reported(OTHER))), "claude"), None);
        let overrides = apply_command_line_provider_sessions(
            &mut derived_elsewhere,
            &[claude(derived(ID)), claude(on_term_c(derived(OTHER)))],
        );
        assert_eq!(overrides.len(), 2);
        assert_eq!(
            derived_elsewhere.workers[0]
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some(ID)
        );
    }

    #[test]
    fn stale_command_line_also_running_elsewhere_keeps_herdr_report() {
        // term-b still shows `codex resume ID` but Herdr reports OTHER there,
        // and term-c really runs `codex resume ID`: term-b's argv is stale.
        let mut inventory = with_second_agent(observed(Some(reported(OTHER))), None);

        let overrides = apply_command_line_provider_sessions(
            &mut inventory,
            &[derived(ID), on_term_c(derived(ID))],
        );

        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].terminal_id, "term-c");
        assert_eq!(inventory.workers[0].provider_session, Some(reported(OTHER)));
        assert_eq!(inventory.panes[0].provider_session, Some(reported(OTHER)));
        assert_eq!(inventory.workers[1].provider_session, Some(reported(ID)));
    }
}
