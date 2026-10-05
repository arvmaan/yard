//! Rich Block Kit cards for the owner DM commands: `help`, `status`,
//! `status <project>`, `blocked` (heading) and `review`.
//!
//! Content comes from the same durable state as [`super::commands`]
//! (titles and states only); the plain `text` fallback is the command's
//! text reply. Navigation buttons carry only an opaque id issued by `nav`
//! (the [`super::actions::ActionRegistry`]); a `None` id (RNG failure)
//! leaves the button out.

use std::fmt::Write as _;

use serde_json::Value;
use yard_domain::ObservedStatus;
use yard_store::{AttentionRecords, AttentionRuntime, AttentionRuntimeRole};

use super::{
    actions::{ACTION_TTL, NavAction},
    blocks::{self, Style},
    commands::{self, ProjectName},
    detector::worker_label,
    message::{OutgoingMessage, clean, since_token, subject_title},
};

/// Issues one navigation id (or `None` when no id could be made).
pub type Nav<'a> = dyn FnMut(NavAction) -> Option<String> + 'a;

const NAME_CHARS: usize = 60;
/// Projects with their own section on the status card.
pub const MAX_STATUS_PROJECTS: usize = 30;
/// Workers with their own section on a project card.
pub const MAX_PROJECT_WORKERS: usize = 20;
/// `action_id` prefix of navigation buttons (`yard_nav_status_<id head>`:
/// unique within a message; Yard reads only the value).
pub const NAV_ACTION_PREFIX: &str = "yard_";

/// Health of a set of agents, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Health {
    Blocked,
    Review,
    Working,
    Idle,
}

impl Health {
    const fn emoji(self) -> &'static str {
        match self {
            Self::Blocked => ":red_circle:",
            Self::Review => ":large_yellow_circle:",
            Self::Working => ":large_green_circle:",
            Self::Idle => ":white_circle:",
        }
    }

    fn of(runtimes: &[&AttentionRuntime]) -> Self {
        runtimes
            .iter()
            .map(|runtime| Self::one(runtime))
            .min()
            .unwrap_or(Self::Idle)
    }

    fn one(runtime: &AttentionRuntime) -> Self {
        if commands::is(runtime, ObservedStatus::Blocked) {
            Self::Blocked
        } else if commands::is(runtime, ObservedStatus::Done) {
            Self::Review
        } else if commands::is(runtime, ObservedStatus::Working) {
            Self::Working
        } else {
            Self::Idle
        }
    }
}

pub(crate) fn nav_button(
    nav: &mut Nav<'_>,
    action: NavAction,
    text: &str,
    style: Style,
) -> Option<Value> {
    let name = action.name();
    let id = nav(action)?;
    let action_id = format!("{NAV_ACTION_PREFIX}{name}_{}", id.get(..8).unwrap_or(&id));
    Some(blocks::button(text, &action_id, &id, style))
}

/// A row of navigation buttons and "Open in Yard".
fn nav_row(nav: &mut Nav<'_>, buttons: Vec<(NavAction, &str, Style)>, ui_url: &str) -> Value {
    let mut elements = buttons
        .into_iter()
        .filter_map(|(action, text, style)| nav_button(nav, action, text, style))
        .collect::<Vec<_>>();
    elements.push(blocks::open_in_yard(ui_url));
    blocks::actions("yard_nav", elements)
}

fn expiry_note() -> String {
    format!(
        "Buttons work once and expire in {} min. \"Always allow\" is never offered.",
        ACTION_TTL.as_secs() / 60
    )
}

/// `help`: what the bot does, with buttons.
#[must_use]
pub fn help_card(nav: &mut Nav<'_>, ui_url: &str) -> OutgoingMessage {
    let rows = [
        ("`status`", "every project at a glance"),
        ("`status <project>`", "one project's agents"),
        ("`blocked`", "blocked agents with answer buttons"),
        ("`review`", "finished work waiting for you"),
        ("`<Project>: question`", "ask that project's orchestrator"),
        ("anything else", "ask the Superintendent"),
        ("`settings` / `quiet`", "quiet hours, mute, next digest"),
        (
            "`mute 2h` · `mute until 9am`",
            "hold notifications (`unmute` ends it)",
        ),
        ("`digest`", "send what is held now"),
    ];
    let pairs = rows
        .iter()
        .map(|(command, what)| (*command, (*what).to_owned()))
        .collect::<Vec<_>>();
    let blocks = vec![
        blocks::header(":wave: Yard in Slack"),
        blocks::section(
            "I tell you when an agent needs you, and answer from Yard's own state. Tap a button or type a command:",
        ),
        blocks::fields(&pairs),
        nav_row(
            nav,
            vec![
                (NavAction::Status, "Status", Style::Primary),
                (NavAction::Blocked, "Blocked", Style::Default),
                (NavAction::Review, "Review", Style::Default),
                (NavAction::AskHint, "Ask the Superintendent", Style::Default),
                (
                    NavAction::Quiet(super::controls::Control::Settings),
                    "Quiet hours",
                    Style::Default,
                ),
            ],
            ui_url,
        ),
        blocks::context(&[
            "Examples: `status Checkout` · `Checkout: what's left?` · `what is everyone working on?`"
                .to_owned(),
            format!(
                "{} Reply in a question card's thread to answer it.",
                expiry_note()
            ),
        ]),
    ];
    OutgoingMessage {
        text: commands::help_text(),
        blocks: blocks::finish(blocks),
    }
}

/// The hint behind "Ask the Superintendent".
#[must_use]
pub fn ask_hint() -> OutgoingMessage {
    let text = "Type your question at the top level of this DM and I'll ask the Superintendent; start with `<Project>:` to ask that project's orchestrator instead. The answer is posted in the thread.";
    OutgoingMessage {
        text: text.to_owned(),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":speech_balloon: {text}")),
            blocks::context(&["Example: `Checkout: what's left before release?`".to_owned()]),
        ]),
    }
}

fn orchestrator_state(runtimes: &[&AttentionRuntime], role: AttentionRuntimeRole) -> String {
    runtimes
        .iter()
        .find(|runtime| runtime.role == role)
        .map_or_else(
            || ":white_circle: not running".to_owned(),
            |runtime| {
                format!(
                    "{} {}",
                    Health::one(runtime).emoji(),
                    commands::state_word(runtime)
                )
            },
        )
}

/// One project on the status card, with its "Details" button.
fn project_section(
    project: &ProjectName,
    runtimes: &[&AttentionRuntime],
    nav: &mut Nav<'_>,
) -> Value {
    let in_project = runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime.project_id.as_deref() == Some(&project.id))
        .collect::<Vec<_>>();
    let workers = in_project
        .iter()
        .copied()
        .filter(|runtime| runtime.role == AttentionRuntimeRole::Assignment)
        .collect::<Vec<_>>();
    let mut text = format!(
        "{} *{}*\nOrchestrator {} · {}",
        Health::of(&in_project).emoji(),
        clean(&project.name, NAME_CHARS),
        orchestrator_state(&in_project, AttentionRuntimeRole::ProjectOrchestrator),
        commands::worker_counts(&workers)
    );
    if let Some(at) = in_project
        .iter()
        .filter_map(|runtime| runtime.last_prompt_at_unix_ms)
        .max()
    {
        let _ = write!(text, " · last activity {}", since_token(at));
    }
    let details = nav_button(
        nav,
        NavAction::ProjectStatus {
            project_id: project.id.clone(),
        },
        "Details",
        Style::Default,
    );
    match details {
        Some(button) => blocks::section_with(&text, button),
        None => blocks::section(&text),
    }
}

/// `status`: one section per project with a "Details" button.
#[must_use]
pub fn status_card(
    records: &AttentionRecords,
    projects: &[ProjectName],
    nav: &mut Nav<'_>,
    ui_url: &str,
) -> OutgoingMessage {
    let text = commands::status_text(records, projects);
    let runtimes = commands::unique(records);
    let projects = commands::visible(records, projects);
    let count = |status| {
        runtimes
            .iter()
            .filter(|runtime| commands::is(runtime, status))
            .count()
    };
    let (blocked, review, working) = (
        count(ObservedStatus::Blocked),
        count(ObservedStatus::Done),
        count(ObservedStatus::Working),
    );
    let mut blocks = vec![
        blocks::header(":bar_chart: Yard status"),
        blocks::context(&[format!(
            ":red_circle: {blocked} blocked · :large_yellow_circle: {review} ready for review · :large_green_circle: {working} working · {} {}",
            projects.len(),
            if projects.len() == 1 {
                "project"
            } else {
                "projects"
            }
        )]),
        blocks::divider(),
    ];
    for project in projects.iter().take(MAX_STATUS_PROJECTS) {
        blocks.push(project_section(project, &runtimes, nav));
    }
    if projects.len() > MAX_STATUS_PROJECTS {
        blocks.push(blocks::context(&[format!(
            "+{} more projects — open Yard.",
            projects.len() - MAX_STATUS_PROJECTS
        )]));
    }
    if projects.is_empty() {
        blocks.push(blocks::section("No projects yet."));
    }
    blocks.push(blocks::divider());
    let mut agents = vec![format!(
        "*Superintendent* {}",
        orchestrator_state(&runtimes, AttentionRuntimeRole::YardOrchestrator)
    )];
    let nodes = runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime.role == AttentionRuntimeRole::CoordinationNode)
        .collect::<Vec<_>>();
    if !nodes.is_empty() {
        agents.push(format!(
            "*Workstreams* {} {}",
            Health::of(&nodes).emoji(),
            commands::worker_counts(&nodes).replacen("worker", "workstream", 1)
        ));
    }
    blocks.push(blocks::section(&agents.join("\n")));
    blocks.push(nav_row(
        nav,
        vec![
            (
                NavAction::Blocked,
                "Blocked",
                if blocked > 0 {
                    Style::Primary
                } else {
                    Style::Default
                },
            ),
            (NavAction::Review, "Review", Style::Default),
        ],
        ui_url,
    ));
    blocks.push(blocks::context(&[
        "Say `status <project>`, `blocked`, `review` or `help`.".to_owned(),
    ]));
    OutgoingMessage {
        text,
        blocks: blocks::finish(blocks),
    }
}

/// One worker on a project card: name, profile, state, task and last
/// prompt; "Answer" when it is blocked.
fn worker_section(runtime: &AttentionRuntime, nav: &mut Nav<'_>) -> Value {
    let health = Health::one(runtime);
    let name = worker_label(runtime)
        .map_or_else(|| "A worker".to_owned(), |name| clean(&name, NAME_CHARS));
    let mut text = format!("{} *{name}*", health.emoji());
    if runtime.display_name.is_some()
        && let Some(profile) = runtime.profile_name.as_deref()
    {
        let _ = write!(text, " · _{}_", clean(profile, NAME_CHARS));
    }
    let _ = write!(text, "\n{}", commands::state_word(runtime));
    if let Some(at) = runtime.last_prompt_at_unix_ms {
        let _ = write!(text, " · last prompt {}", since_token(at));
    }
    if let Some(title) = subject_title(&commands::subject(runtime)) {
        let _ = write!(text, "\n>{title}");
    }
    let answer = (health == Health::Blocked)
        .then(|| commands::runtime_target(runtime))
        .flatten()
        .and_then(|target| {
            nav_button(
                nav,
                NavAction::Answer {
                    target,
                    title: commands::runtime_title(runtime),
                },
                "Answer",
                Style::Primary,
            )
        });
    match answer {
        Some(button) => blocks::section_with(&text, button),
        None => blocks::section(&text),
    }
}

/// `status <project>` for a resolved project.
#[must_use]
pub fn project_card(
    project: &ProjectName,
    records: &AttentionRecords,
    nav: &mut Nav<'_>,
    ui_url: &str,
) -> OutgoingMessage {
    let runtimes = commands::unique(records)
        .into_iter()
        .filter(|runtime| runtime.project_id.as_deref() == Some(&project.id))
        .collect::<Vec<_>>();
    let mut workers = runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime.role == AttentionRuntimeRole::Assignment)
        .collect::<Vec<_>>();
    workers.sort_by_key(|runtime| Health::one(runtime));
    let name = clean(&project.name, NAME_CHARS);
    let mut blocks = vec![
        blocks::header(&format!("{} {name}", Health::of(&runtimes).emoji())),
        blocks::section(&format!(
            "*Orchestrator* {}\n{}",
            orchestrator_state(&runtimes, AttentionRuntimeRole::ProjectOrchestrator),
            commands::worker_counts(&workers)
        )),
        blocks::divider(),
    ];
    for runtime in workers.iter().take(MAX_PROJECT_WORKERS) {
        blocks.push(worker_section(runtime, nav));
    }
    if workers.len() > MAX_PROJECT_WORKERS {
        blocks.push(blocks::context(&[format!(
            "+{} more workers — open Yard.",
            workers.len() - MAX_PROJECT_WORKERS
        )]));
    }
    if workers.is_empty() {
        blocks.push(blocks::section("No workers."));
    }
    blocks.push(nav_row(
        nav,
        vec![
            (NavAction::Status, "All projects", Style::Default),
            (NavAction::Blocked, "Blocked", Style::Default),
        ],
        ui_url,
    ));
    OutgoingMessage {
        text: commands::project_text(project, records),
        blocks: blocks::finish(blocks),
    }
}

/// `review`: workers whose turn finished.
#[must_use]
pub fn review_card(records: &AttentionRecords, nav: &mut Nav<'_>, ui_url: &str) -> OutgoingMessage {
    let mut titles = commands::unique(records)
        .into_iter()
        .filter(|runtime| {
            runtime.role == AttentionRuntimeRole::Assignment
                && commands::is(runtime, ObservedStatus::Done)
        })
        .map(commands::runtime_title)
        .collect::<Vec<_>>();
    titles.sort();
    let text = commands::review_text(records);
    let blocks = if titles.is_empty() {
        vec![
            blocks::section(":white_check_mark: Nothing is waiting for review."),
            nav_row(
                nav,
                vec![
                    (NavAction::Status, "Status", Style::Default),
                    (NavAction::Blocked, "Blocked", Style::Default),
                ],
                ui_url,
            ),
        ]
    } else {
        let mut blocks = vec![blocks::header(&format!(
            ":eyes: Ready for review ({})",
            titles.len()
        ))];
        let shown = titles
            .iter()
            .take(MAX_PROJECT_WORKERS)
            .map(|title| format!("• {title}"))
            .collect::<Vec<_>>();
        blocks.push(blocks::section(&shown.join("\n")));
        if titles.len() > MAX_PROJECT_WORKERS {
            blocks.push(blocks::context(&[format!(
                "+{} more — open Yard.",
                titles.len() - MAX_PROJECT_WORKERS
            )]));
        }
        blocks.push(blocks::context(&[
            "Herdr reports the turn finished; no receipt has been recorded. Review in Yard."
                .to_owned(),
        ]));
        blocks.push(nav_row(
            nav,
            vec![(NavAction::Status, "Status", Style::Default)],
            ui_url,
        ));
        blocks
    };
    OutgoingMessage {
        text,
        blocks: blocks::finish(blocks),
    }
}

/// The heading `blocked` posts before the prompt cards.
#[must_use]
pub fn blocked_heading(count: usize, nav: &mut Nav<'_>, ui_url: &str) -> OutgoingMessage {
    if count == 0 {
        return OutgoingMessage {
            text: "Nothing is blocked.".to_owned(),
            blocks: blocks::finish(vec![
                blocks::section(":white_check_mark: Nothing is blocked."),
                nav_row(
                    nav,
                    vec![
                        (NavAction::Status, "Status", Style::Default),
                        (NavAction::Review, "Review", Style::Default),
                    ],
                    ui_url,
                ),
            ]),
        };
    }
    let text = match count {
        1 => "1 agent is blocked:".to_owned(),
        count => format!("{count} agents are blocked:"),
    };
    OutgoingMessage {
        blocks: blocks::finish(vec![
            blocks::header(&format!(":raised_hand: {}", text.trim_end_matches(':'))),
            blocks::context(&[format!(
                "Each prompt follows with its answer buttons. {}",
                expiry_note()
            )]),
        ]),
        text,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use yard_domain::ObservedStatus;
    use yard_store::{AttentionRecords, AttentionRuntimeRole};

    use super::{
        MAX_STATUS_PROJECTS, blocked_heading, help_card, project_card, review_card, status_card,
    };
    use crate::slack::{
        actions::NavAction,
        commands::{
            ProjectName,
            tests::{projects, records, runtime},
        },
    };

    const URL: &str = "https://yard.example.test/";

    /// Records every issued navigation action; ids are numbered tokens.
    fn issuer(issued: &mut Vec<NavAction>) -> impl FnMut(NavAction) -> Option<String> + '_ {
        move |action| {
            issued.push(action);
            Some(format!("{:032x}", issued.len()))
        }
    }

    fn buttons(blocks: &Value) -> Vec<(String, Option<String>, Option<String>)> {
        let mut found = Vec::new();
        for block in blocks.as_array().unwrap() {
            let elements = block["elements"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .chain(block.get("accessory").cloned());
            for element in elements.filter(|element| element["type"] == "button") {
                found.push((
                    element["text"]["text"].as_str().unwrap().to_owned(),
                    element["value"].as_str().map(str::to_owned),
                    element["url"].as_str().map(str::to_owned),
                ));
            }
        }
        found
    }

    fn within_limits(blocks: &Value) {
        let blocks = blocks.as_array().unwrap();
        assert!(blocks.len() <= 50, "{} blocks", blocks.len());
        for block in blocks {
            let text = block["text"]["text"].as_str().unwrap_or_default();
            assert!(text.chars().count() <= 3_000);
            assert!(block["fields"].as_array().map_or(0, Vec::len) <= 10);
            assert!(block["elements"].as_array().map_or(0, Vec::len) <= 25);
        }
        let all = Value::Array(blocks.clone()).to_string().to_lowercase();
        assert!(!all.contains("always allow\","), "never a button for it");
    }

    #[test]
    fn help_offers_navigation_buttons_with_opaque_values_only() {
        let mut issued = Vec::new();
        let card = help_card(&mut issuer(&mut issued), URL);
        within_limits(&card.blocks);
        assert_eq!(card.blocks[0]["type"], "header");
        let found = buttons(&card.blocks);
        let labels = found
            .iter()
            .map(|(text, ..)| text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "Status",
                "Blocked",
                "Review",
                "Ask the Superintendent",
                "Quiet hours",
                "Open in Yard"
            ]
        );
        assert_eq!(
            issued,
            [
                NavAction::Status,
                NavAction::Blocked,
                NavAction::Review,
                NavAction::AskHint,
                NavAction::Quiet(crate::slack::controls::Control::Settings)
            ]
        );
        for (_, value, url) in &found[..5] {
            assert!(crate::slack::inbound::is_action_token(
                value.as_deref().unwrap()
            ));
            assert!(url.is_none());
        }
        assert_eq!(found[5].2.as_deref(), Some(URL));
        assert!(card.text.contains("`status <project>`"), "plain fallback");
        // Without ids there are no navigation buttons, only the link.
        let bare = help_card(&mut |_| None, URL);
        assert_eq!(buttons(&bare.blocks).len(), 1);
    }

    #[test]
    fn status_has_a_section_and_details_button_per_project() {
        let mut records = records();
        // Telemetry: the newest prompt across its runtimes is its activity.
        records.runtimes[1].last_prompt_at_unix_ms = Some(1_695_900_000_000);
        records.runtimes[4].last_prompt_at_unix_ms = Some(1_695_903_600_000);
        let mut issued = Vec::new();
        let card = status_card(&records, &projects(), &mut issuer(&mut issued), URL);
        within_limits(&card.blocks);
        let all = card.blocks.to_string();
        assert!(all.contains(":red_circle: *Telemetry*"), "{all}");
        assert!(
            all.contains(" · last activity <!date^1695903600^{ago}"),
            "{all}"
        );
        assert_eq!(all.matches("last activity").count(), 1, "{all}");
        assert!(all.contains("*Checkout &lt;!channel&gt;*"), "{all}");
        assert!(
            !all.contains("<!channel>") && !all.contains("Archived"),
            "{all}"
        );
        assert!(
            !all.contains("abc123") && !all.contains("Secret details"),
            "{all}"
        );
        assert!(all.contains(":red_circle: 2 blocked"), "{all}");
        assert_eq!(
            &issued[..2],
            [
                NavAction::ProjectStatus {
                    project_id: "p-check".to_owned()
                },
                NavAction::ProjectStatus {
                    project_id: "p-token".to_owned()
                },
            ]
        );
        assert!(issued.contains(&NavAction::Blocked) && issued.contains(&NavAction::Review));
        assert!(card.text.starts_with("*Yard status*"), "plain fallback");
    }

    #[test]
    fn status_paginates_many_projects_within_slack_limits() {
        let projects = (0..120)
            .map(|n| ProjectName {
                id: format!("p{n:03}"),
                name: format!("Project {n:03} {}", "x".repeat(200)),
            })
            .collect::<Vec<_>>();
        let records = AttentionRecords {
            runtimes: Vec::new(),
            commands: Vec::new(),
            visible_project_ids: projects.iter().map(|project| project.id.clone()).collect(),
            visible_node_ids: std::collections::HashSet::new(),
        };
        let mut issued = Vec::new();
        let card = status_card(&records, &projects, &mut issuer(&mut issued), URL);
        within_limits(&card.blocks);
        let all = card.blocks.to_string();
        assert!(all.contains(&format!(
            "+{} more projects — open Yard.",
            120 - MAX_STATUS_PROJECTS
        )));
        assert_eq!(issued.len(), MAX_STATUS_PROJECTS + 2);
    }

    #[test]
    fn project_cards_list_workers_with_answer_buttons_for_blocked_ones() {
        let mut records = records();
        records.runtimes[1].display_name = Some("Ada *x* <@U1>".to_owned());
        records.runtimes[1].last_prompt_at_unix_ms = Some(1_695_900_000_000);
        let project = projects().remove(0);
        let mut issued = Vec::new();
        let card = project_card(&project, &records, &mut issuer(&mut issued), URL);
        within_limits(&card.blocks);
        let all = card.blocks.to_string();
        assert_eq!(card.blocks[0]["text"]["text"], ":red_circle: Telemetry");
        assert!(all.contains("*Ada ∗x∗ &lt;@U1&gt;* · _Claude_"), "{all}");
        assert!(all.contains(">Objective of w1"), "{all}");
        assert!(all.contains("last prompt <!date^1695900000^"), "{all}");
        assert!(!all.contains("abc123"), "{all}");
        let answers = issued
            .iter()
            .filter(|action| matches!(action, NavAction::Answer { .. }))
            .collect::<Vec<_>>();
        assert_eq!(answers.len(), 1, "only the blocked worker: {issued:?}");
        let labels = buttons(&card.blocks)
            .into_iter()
            .map(|(text, ..)| text)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["Answer", "All projects", "Blocked", "Open in Yard"]
        );
        assert!(
            card.text.contains("Orchestrator: working"),
            "plain fallback"
        );
    }

    #[test]
    fn review_and_blocked_cards() {
        let mut issued = Vec::new();
        let card = review_card(&records(), &mut issuer(&mut issued), URL);
        within_limits(&card.blocks);
        assert_eq!(
            card.blocks[0]["text"]["text"],
            ":eyes: Ready for review (1)"
        );
        assert!(
            card.blocks
                .to_string()
                .contains("• Telemetry · Claude (Objective of w2)")
        );
        assert!(
            buttons(&card.blocks)
                .iter()
                .any(|(_, _, url)| url.as_deref() == Some(URL))
        );
        let empty = AttentionRecords {
            runtimes: vec![runtime(
                AttentionRuntimeRole::Assignment,
                "w9",
                Some(("p-token", "Telemetry")),
                ObservedStatus::Working,
            )],
            ..records()
        };
        let none = review_card(&empty, &mut issuer(&mut issued), URL);
        assert_eq!(none.text, "Nothing is waiting for review.");
        let heading = blocked_heading(2, &mut issuer(&mut issued), URL);
        assert_eq!(heading.text, "2 agents are blocked:");
        assert_eq!(
            heading.blocks[0]["text"]["text"],
            ":raised_hand: 2 agents are blocked"
        );
        let nothing = blocked_heading(0, &mut issuer(&mut issued), URL);
        assert_eq!(nothing.text, "Nothing is blocked.");
        assert_eq!(buttons(&nothing.blocks).len(), 3);
    }
}
