//! Owner DM commands (phase 2): instant, deterministic answers from durable
//! state (no LLM), and where a free-form question goes.
//!
//! `help`, `status`, `status <project>`, `blocked` and `review` read the
//! same read-only attention records as the notifier ([`AttentionRecords`])
//! plus the project list, and show titles and states only. Anything else is
//! a question: `<Project name>: …` goes to that project's orchestrator,
//! everything else to the Superintendent (Yard orchestrator).

use std::{collections::HashSet, fmt::Write as _};

use yard_domain::{ObservedStatus, RuntimeObservationState, RuntimeProcessState};
use yard_store::{AttentionRecords, AttentionRuntime, AttentionRuntimeRole};

use super::{
    actions::AgentTarget,
    detector::{AttentionEvent, Subject, worker_label},
    message::{clean, subject_label},
    prompt::cap,
};

const NAME_CHARS: usize = 60;
/// Longest list a command prints before "and N more".
const MAX_LIST: usize = 20;
/// `blocked` posts at most this many prompt cards.
pub const MAX_BLOCKED_CARDS: usize = 5;
/// Slack section text is capped at 3,000 characters.
const MAX_REPLY_CHARS: usize = 2_900;

/// A visible (not archived, not deleted) project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectName {
    pub id: String,
    pub name: String,
}

/// What an owner DM asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Status,
    /// `status <query>`: the query as typed.
    ProjectStatus(String),
    Blocked,
    Review,
    /// A free-form question for an orchestrator.
    Ask {
        target: AgentTarget,
        /// Who it goes to, cleaned for display.
        to: String,
        question: String,
    },
    /// `<name>: …` where several projects share the name.
    AmbiguousProject(String),
}

fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A command word, ignoring case and trailing `?`, `.` or `!`.
fn word(text: &str) -> String {
    normalized(text.trim_end_matches(['?', '.', '!']))
}

/// Parse one owner DM. `projects` are the visible projects, for the
/// `<Project>:` prefix.
#[must_use]
pub fn parse(text: &str, projects: &[ProjectName]) -> Command {
    let text = text.trim();
    let command = word(text);
    match command.as_str() {
        "help" | "commands" | "" => return Command::Help,
        "status" => return Command::Status,
        "blocked" => return Command::Blocked,
        "review" => return Command::Review,
        _ => {}
    }
    if let Some(query) = command.strip_prefix("status ") {
        // Take the query from the original text so display keeps its case.
        let offset = text.len() - text.trim_start_matches(|c: char| !c.is_whitespace()).len();
        let original = text[offset..]
            .trim()
            .trim_end_matches(['?', '.', '!'])
            .trim();
        let query = if original.is_empty() { query } else { original };
        return Command::ProjectStatus(query.to_owned());
    }
    if let Some((prefix, rest)) = text.split_once(':') {
        let wanted = normalized(prefix);
        let matches = projects
            .iter()
            .filter(|project| !wanted.is_empty() && normalized(&project.name) == wanted)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [project] => {
                let question = rest.trim();
                if question.is_empty() {
                    return Command::Help;
                }
                return Command::Ask {
                    target: AgentTarget::ProjectOrchestrator {
                        project_id: project.id.clone(),
                    },
                    to: format!("the {} orchestrator", clean(&project.name, NAME_CHARS)),
                    question: question.to_owned(),
                };
            }
            [] => {}
            _ => return Command::AmbiguousProject(clean(prefix.trim(), NAME_CHARS)),
        }
    }
    Command::Ask {
        target: AgentTarget::YardOrchestrator,
        to: "the Superintendent".to_owned(),
        question: text.to_owned(),
    }
}

/// `help`.
#[must_use]
pub fn help_text() -> String {
    [
        "*Yard in Slack*",
        "• `status` — every project at a glance",
        "• `status <project>` — one project's agents",
        "• `blocked` — each blocked agent with its answer buttons",
        "• `review` — work that finished its turn and waits for your review",
        "• `<Project>: question` — ask that project's orchestrator",
        "• anything else at the top level — ask the Superintendent; the answer is posted in the thread",
        "Buttons answer once and expire in 5 minutes; \"always allow\" is never offered. Reply in a question card's thread to answer it; other thread replies are not sent.",
    ]
    .join("\n")
}

/// Observed by Herdr and the process has not exited.
fn live(runtime: &AttentionRuntime) -> bool {
    runtime.observation_state == RuntimeObservationState::Observed
        && runtime.process_state != RuntimeProcessState::Exited
}

fn state_word(runtime: &AttentionRuntime) -> &'static str {
    if runtime.process_state == RuntimeProcessState::Exited {
        return "exited";
    }
    if runtime.observation_state != RuntimeObservationState::Observed {
        return "not observed";
    }
    match runtime.status {
        ObservedStatus::Idle => "idle",
        ObservedStatus::Working => "working",
        ObservedStatus::Blocked => "blocked (waiting on a prompt)",
        ObservedStatus::Done => "finished its turn (ready for review)",
        ObservedStatus::Unknown => "unknown",
    }
}

fn is(runtime: &AttentionRuntime, status: ObservedStatus) -> bool {
    live(runtime) && runtime.status == status
}

/// One entry per worker (the records may list a worker twice).
fn unique(records: &AttentionRecords) -> Vec<&AttentionRuntime> {
    let mut seen = HashSet::new();
    records
        .runtimes
        .iter()
        .filter(|runtime| seen.insert(runtime.worker_id.as_str()))
        .collect()
}

fn subject(runtime: &AttentionRuntime) -> Subject {
    match runtime.role {
        AttentionRuntimeRole::Assignment => Subject::Worker {
            profile_name: worker_label(runtime),
            objective: runtime.objective.clone(),
        },
        AttentionRuntimeRole::ProjectOrchestrator => Subject::ProjectOrchestrator,
        AttentionRuntimeRole::YardOrchestrator => Subject::YardOrchestrator,
        AttentionRuntimeRole::CoordinationNode => Subject::Workstream {
            name: runtime.node_name.clone(),
        },
    }
}

/// Display title (cleaned): `Project · agent`.
#[must_use]
pub fn runtime_title(runtime: &AttentionRuntime) -> String {
    let label = subject_label(&subject(runtime));
    match (runtime.role, runtime.project_name.as_deref()) {
        (
            AttentionRuntimeRole::Assignment | AttentionRuntimeRole::ProjectOrchestrator,
            Some(project),
        ) => format!("{} · {label}", clean(project, NAME_CHARS)),
        _ => label,
    }
}

/// Display title (cleaned) of a notification's agent: `Project · agent`.
#[must_use]
pub fn event_title(event: &AttentionEvent) -> String {
    let label = subject_label(&event.subject);
    match (&event.subject, event.project_name.as_deref()) {
        (Subject::Worker { .. } | Subject::ProjectOrchestrator, Some(project)) => {
            format!("{} · {label}", clean(project, NAME_CHARS))
        }
        _ => label,
    }
}

/// The agent Slack would answer for `runtime`.
#[must_use]
pub fn runtime_target(runtime: &AttentionRuntime) -> Option<AgentTarget> {
    Some(match runtime.role {
        AttentionRuntimeRole::Assignment => AgentTarget::Assignment {
            project_id: runtime.project_id.clone()?,
            assignment_id: runtime.assignment_id.clone()?,
        },
        AttentionRuntimeRole::ProjectOrchestrator => AgentTarget::ProjectOrchestrator {
            project_id: runtime.project_id.clone()?,
        },
        AttentionRuntimeRole::YardOrchestrator => AgentTarget::YardOrchestrator,
        AttentionRuntimeRole::CoordinationNode => AgentTarget::CoordinationNode {
            node_id: runtime.node_id.clone()?,
        },
    })
}

/// Blocked agents, projects first then the Superintendent and workstreams.
#[must_use]
pub fn blocked_agents(records: &AttentionRecords) -> Vec<(AgentTarget, String)> {
    let mut blocked = unique(records)
        .into_iter()
        .filter(|runtime| is(runtime, ObservedStatus::Blocked))
        .filter_map(|runtime| Some((runtime_target(runtime)?, runtime_title(runtime))))
        .collect::<Vec<_>>();
    blocked.sort_by(|left, right| left.1.cmp(&right.1));
    blocked
}

fn more(lines: &mut Vec<String>, total: usize) {
    if total > MAX_LIST {
        lines.truncate(MAX_LIST);
        lines.push(format!("…and {} more — open Yard.", total - MAX_LIST));
    }
}

/// Visible projects sorted by name.
fn visible<'a>(records: &AttentionRecords, projects: &'a [ProjectName]) -> Vec<&'a ProjectName> {
    let mut visible = projects
        .iter()
        .filter(|project| records.visible_project_ids.contains(&project.id))
        .collect::<Vec<_>>();
    visible.sort_by_key(|project| normalized(&project.name));
    visible
}

fn worker_counts(workers: &[&AttentionRuntime]) -> String {
    if workers.is_empty() {
        return "no workers".to_owned();
    }
    let count = |status| workers.iter().filter(|runtime| is(runtime, status)).count();
    let mut parts = Vec::new();
    for (status, label) in [
        (ObservedStatus::Blocked, "blocked"),
        (ObservedStatus::Working, "working"),
        (ObservedStatus::Done, "ready for review"),
        (ObservedStatus::Idle, "idle"),
    ] {
        let n = count(status);
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    let other = workers.len()
        - [
            ObservedStatus::Blocked,
            ObservedStatus::Working,
            ObservedStatus::Done,
            ObservedStatus::Idle,
        ]
        .into_iter()
        .map(count)
        .sum::<usize>();
    if other > 0 {
        parts.push(format!("{other} not observed"));
    }
    format!(
        "{} {}: {}",
        workers.len(),
        if workers.len() == 1 {
            "worker"
        } else {
            "workers"
        },
        parts.join(", ")
    )
}

const FOOTER: &str = "Say `blocked`, `review`, `status <project>` or `help`.";

/// `status`: every visible project, the Superintendent and workstreams.
#[must_use]
pub fn status_text(records: &AttentionRecords, projects: &[ProjectName]) -> String {
    let runtimes = unique(records);
    let projects = visible(records, projects);
    let mut lines = Vec::new();
    for project in &projects {
        let in_project = |role| {
            runtimes
                .iter()
                .copied()
                .filter(|runtime| {
                    runtime.role == role && runtime.project_id.as_deref() == Some(&project.id)
                })
                .collect::<Vec<_>>()
        };
        let orchestrator = in_project(AttentionRuntimeRole::ProjectOrchestrator)
            .first()
            .map_or("not running", |runtime| state_word(runtime));
        let workers = in_project(AttentionRuntimeRole::Assignment);
        lines.push(format!(
            "• *{}* — orchestrator {orchestrator} · {}",
            clean(&project.name, NAME_CHARS),
            worker_counts(&workers)
        ));
    }
    more(&mut lines, projects.len());
    if projects.is_empty() {
        lines.push("No projects.".to_owned());
    }
    let superintendent = runtimes
        .iter()
        .find(|runtime| runtime.role == AttentionRuntimeRole::YardOrchestrator)
        .map_or("not running", |runtime| state_word(runtime));
    lines.push(format!("Superintendent: {superintendent}"));
    let nodes = runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime.role == AttentionRuntimeRole::CoordinationNode)
        .collect::<Vec<_>>();
    if !nodes.is_empty() {
        let summary = worker_counts(&nodes).replacen("worker", "workstream", 1);
        lines.push(format!("Workstreams: {summary}"));
    }
    let blocked = runtimes
        .iter()
        .filter(|runtime| is(runtime, ObservedStatus::Blocked))
        .count();
    let review = runtimes
        .iter()
        .filter(|runtime| {
            runtime.role == AttentionRuntimeRole::Assignment && is(runtime, ObservedStatus::Done)
        })
        .count();
    let mut text = String::from("*Yard status*\n");
    text.push_str(&lines.join("\n"));
    let _ = write!(
        text,
        "\nBlocked: {blocked} · Ready for review: {review}. {FOOTER}"
    );
    cap(&text, MAX_REPLY_CHARS)
}

/// The project `query` names: an exact (case-insensitive) name, else the
/// only name containing it. `Err` lists the candidates (maybe none).
///
/// # Errors
///
/// Returns the matching projects when there is not exactly one.
pub fn find_project<'a>(
    query: &str,
    records: &AttentionRecords,
    projects: &'a [ProjectName],
) -> Result<&'a ProjectName, Vec<&'a ProjectName>> {
    let wanted = normalized(query);
    let projects = visible(records, projects);
    let exact = projects
        .iter()
        .copied()
        .filter(|project| normalized(&project.name) == wanted)
        .collect::<Vec<_>>();
    let candidates = if exact.is_empty() {
        projects
            .iter()
            .copied()
            .filter(|project| !wanted.is_empty() && normalized(&project.name).contains(&wanted))
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [project] => Ok(project),
        _ => Err(candidates),
    }
}

/// `status <project>`.
#[must_use]
pub fn project_status_text(
    query: &str,
    records: &AttentionRecords,
    projects: &[ProjectName],
) -> String {
    let project = match find_project(query, records, projects) {
        Ok(project) => project,
        Err(candidates) if candidates.is_empty() => {
            let names = visible(records, projects)
                .iter()
                .take(MAX_LIST)
                .map(|project| clean(&project.name, NAME_CHARS))
                .collect::<Vec<_>>();
            let names = if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            };
            return cap(
                &format!(
                    "No project matches \"{}\". Projects: {names}.",
                    clean(query, NAME_CHARS)
                ),
                MAX_REPLY_CHARS,
            );
        }
        Err(candidates) => {
            let names = candidates
                .iter()
                .take(MAX_LIST)
                .map(|project| clean(&project.name, NAME_CHARS))
                .collect::<Vec<_>>()
                .join(", ");
            return cap(
                &format!(
                    "\"{}\" matches several projects: {names}. Say `status` and the full name.",
                    clean(query, NAME_CHARS)
                ),
                MAX_REPLY_CHARS,
            );
        }
    };
    let runtimes = unique(records)
        .into_iter()
        .filter(|runtime| runtime.project_id.as_deref() == Some(&project.id))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let orchestrator = runtimes
        .iter()
        .find(|runtime| runtime.role == AttentionRuntimeRole::ProjectOrchestrator)
        .map_or("not running", |runtime| state_word(runtime));
    lines.push(format!("• Orchestrator: {orchestrator}"));
    let workers = runtimes
        .iter()
        .filter(|runtime| runtime.role == AttentionRuntimeRole::Assignment)
        .map(|runtime| {
            format!(
                "• {}: {}",
                subject_label(&subject(runtime)),
                state_word(runtime)
            )
        })
        .collect::<Vec<_>>();
    let total = workers.len();
    let mut workers = workers;
    more(&mut workers, total);
    if workers.is_empty() {
        workers.push("• No workers.".to_owned());
    }
    lines.extend(workers);
    cap(
        &format!(
            "*{}*\n{}\n{FOOTER}",
            clean(&project.name, NAME_CHARS),
            lines.join("\n")
        ),
        MAX_REPLY_CHARS,
    )
}

/// `review`: assignment workers whose turn finished.
#[must_use]
pub fn review_text(records: &AttentionRecords) -> String {
    let mut lines = unique(records)
        .into_iter()
        .filter(|runtime| {
            runtime.role == AttentionRuntimeRole::Assignment && is(runtime, ObservedStatus::Done)
        })
        .map(|runtime| format!("• {}", runtime_title(runtime)))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "Nothing is waiting for review.".to_owned();
    }
    lines.sort();
    let total = lines.len();
    more(&mut lines, total);
    cap(
        &format!(
            "*Ready for review ({total})* — Herdr reports the turn finished; no receipt has been recorded. Review in Yard.\n{}",
            lines.join("\n")
        ),
        MAX_REPLY_CHARS,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use yard_domain::{ObservedStatus, RuntimeObservationState, RuntimeProcessState};
    use yard_store::{AttentionRecords, AttentionRuntime, AttentionRuntimeRole};

    use std::collections::HashSet;

    use super::{
        Command, ProjectName, blocked_agents, help_text, parse, project_status_text, review_text,
        runtime_title, status_text,
    };
    use crate::slack::actions::AgentTarget;

    pub(crate) fn runtime(
        role: AttentionRuntimeRole,
        worker: &str,
        project: Option<(&str, &str)>,
        status: ObservedStatus,
    ) -> AttentionRuntime {
        AttentionRuntime {
            role,
            worker_id: worker.to_owned(),
            project_id: project.map(|(id, _)| id.to_owned()),
            project_name: project.map(|(_, name)| name.to_owned()),
            assignment_id: (role == AttentionRuntimeRole::Assignment)
                .then(|| format!("assignment-{worker}")),
            objective: (role == AttentionRuntimeRole::Assignment)
                .then(|| format!("Objective of {worker}\nSecret details token=abc123")),
            node_id: (role == AttentionRuntimeRole::CoordinationNode)
                .then(|| format!("node-{worker}")),
            node_name: (role == AttentionRuntimeRole::CoordinationNode)
                .then(|| "Release".to_owned()),
            profile_name: Some("Claude".to_owned()),
            display_name: None,
            status,
            process_state: RuntimeProcessState::Running,
            observation_state: RuntimeObservationState::Observed,
            state_change_sequence: 1,
            provider_session: None,
            last_prompt_automatic: false,
            last_prompt_at_unix_ms: None,
        }
    }

    pub(crate) fn projects() -> Vec<ProjectName> {
        vec![
            ProjectName {
                id: "p-token".to_owned(),
                name: "Telemetry".to_owned(),
            },
            ProjectName {
                id: "p-check".to_owned(),
                name: "Checkout <!channel>".to_owned(),
            },
            ProjectName {
                id: "p-gone".to_owned(),
                name: "Archived".to_owned(),
            },
        ]
    }

    pub(crate) fn records() -> AttentionRecords {
        use AttentionRuntimeRole as R;
        use ObservedStatus as S;
        let token = Some(("p-token", "Telemetry"));
        let check = Some(("p-check", "Checkout <!channel>"));
        let mut exited = runtime(R::Assignment, "w5", check, S::Working);
        exited.process_state = RuntimeProcessState::Exited;
        let mut unobserved = runtime(R::Assignment, "w6", check, S::Blocked);
        unobserved.observation_state = RuntimeObservationState::Missing;
        AttentionRecords {
            runtimes: vec![
                runtime(R::ProjectOrchestrator, "o1", token, S::Working),
                runtime(R::Assignment, "w1", token, S::Blocked),
                runtime(R::Assignment, "w1", token, S::Blocked),
                runtime(R::Assignment, "w2", token, S::Done),
                runtime(R::Assignment, "w3", token, S::Working),
                runtime(R::ProjectOrchestrator, "o2", check, S::Idle),
                exited,
                unobserved,
                runtime(R::YardOrchestrator, "y1", None, S::Idle),
                runtime(R::CoordinationNode, "n1", None, S::Blocked),
            ],
            commands: Vec::new(),
            visible_project_ids: ["p-token", "p-check"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            visible_node_ids: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn commands_are_exact_words_and_questions_route_by_project_prefix() {
        let projects = projects();
        for (text, expected) in [
            ("help", Command::Help),
            ("  HELP ", Command::Help),
            ("status", Command::Status),
            ("Status?", Command::Status),
            ("blocked", Command::Blocked),
            ("review.", Command::Review),
            (
                "status telemetry",
                Command::ProjectStatus("telemetry".to_owned()),
            ),
            (
                "status  Checkout <!channel>?",
                Command::ProjectStatus("Checkout <!channel>".to_owned()),
            ),
        ] {
            assert_eq!(parse(text, &projects), expected, "{text}");
        }
        let Command::Ask {
            target,
            to,
            question,
        } = parse("telemetry : what's left?", &projects)
        else {
            panic!("a question");
        };
        assert_eq!(
            target,
            AgentTarget::ProjectOrchestrator {
                project_id: "p-token".to_owned()
            }
        );
        assert_eq!(to, "the Telemetry orchestrator");
        assert_eq!(question, "what's left?");
        // An unknown prefix, a statement, and "blocked?" in a sentence go to
        // the Superintendent unchanged.
        for text in [
            "Note: ship it today",
            "what is blocked right now?",
            "status of the release?",
        ] {
            let command = parse(text, &projects);
            if text.starts_with("status ") {
                assert!(matches!(command, Command::ProjectStatus(_)), "{text}");
                continue;
            }
            assert_eq!(
                command,
                Command::Ask {
                    target: AgentTarget::YardOrchestrator,
                    to: "the Superintendent".to_owned(),
                    question: text.to_owned(),
                },
                "{text}"
            );
        }
        // `Name:` with nothing after it is help, never an empty prompt.
        assert_eq!(parse("Telemetry:", &projects), Command::Help);
        // Two projects with the same name: nothing is sent.
        let mut twins = projects.clone();
        twins.push(ProjectName {
            id: "p-token-2".to_owned(),
            name: "telemetry".to_owned(),
        });
        assert_eq!(
            parse("Telemetry: hi", &twins),
            Command::AmbiguousProject("Telemetry".to_owned())
        );
        assert!(help_text().contains("`blocked`"));
    }

    #[test]
    fn named_workers_use_their_display_name_in_titles() {
        let mut named = runtime(
            AttentionRuntimeRole::Assignment,
            "w-named",
            Some(("p-token", "Telemetry")),
            ObservedStatus::Done,
        );
        named.display_name = Some("BAR CDK".to_owned());
        let title = runtime_title(&named);
        assert!(title.starts_with("Telemetry · BAR CDK ("), "{title}");
        assert!(!title.contains("Claude"), "{title}");
        let unnamed = runtime(
            AttentionRuntimeRole::Assignment,
            "w-plain",
            Some(("p-token", "Telemetry")),
            ObservedStatus::Done,
        );
        assert!(runtime_title(&unnamed).starts_with("Telemetry · Claude ("));

        let review = review_text(&AttentionRecords {
            runtimes: vec![named.clone(), unnamed],
            commands: Vec::new(),
            visible_project_ids: std::iter::once("p-token".to_owned()).collect(),
            visible_node_ids: HashSet::new(),
        });
        assert!(review.contains("• Telemetry · BAR CDK ("), "{review}");
        assert!(review.contains("• Telemetry · Claude ("), "{review}");

        named.status = ObservedStatus::Blocked;
        let blocked = blocked_agents(&AttentionRecords {
            runtimes: vec![named],
            commands: Vec::new(),
            visible_project_ids: std::iter::once("p-token".to_owned()).collect(),
            visible_node_ids: HashSet::new(),
        });
        assert_eq!(blocked.len(), 1);
        assert!(blocked[0].1.contains("BAR CDK"), "{:?}", blocked[0].1);
    }

    #[test]
    fn named_worker_titles_cannot_inject_slack_formatting_links_or_mentions() {
        let mut named = runtime(
            AttentionRuntimeRole::Assignment,
            "w-named",
            Some(("p-token", "Telemetry")),
            ObservedStatus::Blocked,
        );
        named.display_name = Some("*x* _y_ ~z~ `c` <!channel> https://a.b".to_owned());
        let title = runtime_title(&named);
        for forbidden in ['*', '_', '~', '`', '<', '>'] {
            assert!(!title.contains(forbidden), "{forbidden:?} in {title}");
        }
        assert!(!title.contains("://"), "{title}");
        assert!(title.contains("&lt;!channel&gt;"), "{title}");
        let card = crate::slack::cards::prompt_card(
            &title,
            &crate::slack::prompt::ParsedScreen::Unparsed {
                excerpt: "x".to_owned(),
            },
            &[],
        );
        let body = card.blocks[0]["text"]["text"].as_str().unwrap();
        // Only the card's own bold markers remain.
        assert!(body.starts_with(&format!("*{title}* is blocked")), "{body}");
        assert_eq!(body.matches('*').count(), 2, "{body}");
    }

    #[test]
    fn status_commands_show_titles_and_states_from_durable_state_only() {
        let (records, projects) = (records(), projects());
        let status = status_text(&records, &projects);
        assert!(status.starts_with("*Yard status*"), "{status}");
        assert!(
            status.contains(
                "• *Telemetry* — orchestrator working · 3 workers: 1 blocked, 1 working, 1 ready for review"
            ),
            "{status}"
        );
        assert!(
            status.contains(
                "• *Checkout &lt;!channel&gt;* — orchestrator idle · 2 workers: 2 not observed"
            ),
            "{status}"
        );
        assert!(!status.contains("Archived"), "archived projects are hidden");
        assert!(status.contains("Superintendent: idle"), "{status}");
        assert!(
            status.contains("Workstreams: 1 workstream: 1 blocked"),
            "{status}"
        );
        // The duplicate row, the exited and the unobserved worker are not
        // counted as blocked.
        assert!(
            status.contains("Blocked: 2 · Ready for review: 1."),
            "{status}"
        );
        assert!(!status.contains("abc123") && !status.contains("Secret details"));

        let project = project_status_text("tele", &records, &projects);
        assert!(
            project.starts_with("*Telemetry*\n• Orchestrator: working"),
            "{project}"
        );
        assert!(
            project.contains("• Claude (Objective of w1): blocked (waiting on a prompt)"),
            "{project}"
        );
        assert!(
            project.contains("• Claude (Objective of w2): finished its turn (ready for review)"),
            "{project}"
        );
        assert_eq!(
            project.matches("Objective of w1").count(),
            1,
            "deduplicated"
        );
        let checkout = project_status_text("checkout <!channel>", &records, &projects);
        assert!(checkout.contains(": exited") && checkout.contains(": not observed"));
        let missing = project_status_text("Nope", &records, &projects);
        assert!(
            missing.contains(
                "No project matches \"Nope\". Projects: Checkout &lt;!channel&gt;, Telemetry."
            ),
            "{missing}"
        );
        assert!(
            project_status_text("Archived", &records, &projects).contains("No project matches")
        );
        let several = project_status_text("t", &records, &projects);
        assert!(several.contains("matches several projects"), "{several}");

        let review = review_text(&records);
        assert!(review.contains("*Ready for review (1)*"), "{review}");
        assert!(
            review.contains("• Telemetry · Claude (Objective of w2)"),
            "{review}"
        );
        let mut quiet = records.clone();
        quiet
            .runtimes
            .retain(|runtime| runtime.status != ObservedStatus::Done);
        assert_eq!(review_text(&quiet), "Nothing is waiting for review.");

        let blocked = blocked_agents(&records);
        assert_eq!(
            blocked,
            [
                (
                    AgentTarget::Assignment {
                        project_id: "p-token".to_owned(),
                        assignment_id: "assignment-w1".to_owned(),
                    },
                    "Telemetry · Claude (Objective of w1)".to_owned()
                ),
                (
                    AgentTarget::CoordinationNode {
                        node_id: "node-n1".to_owned()
                    },
                    "Workstream Release".to_owned()
                ),
            ]
        );
    }
}
