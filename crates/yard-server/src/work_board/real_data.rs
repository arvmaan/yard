//! Real-data check of the board text (ignored; run by hand). Points the
//! board code at a read-only GET snapshot of a live Yard and prints every
//! card's title and three lines, where it lands, and the Older-work and
//! helper counts, so reviewers judge real wording. The snapshot and the
//! output stay in private temp directories; nothing real is committed.
//!
//! `YARD_BOARD_REAL_SNAPSHOT=<dir> YARD_BOARD_REAL_OUT=<file> cargo test -p
//! yard-server --lib real_data -- --ignored --nocapture`, where `<dir>`
//! holds `api/{now_ms,projects.json,workers.json,sessions.json}`,
//! `api/assignments/*.json`, `api/reports/*.json`,
//! `api/inventories/*.json` and the transcript tails under
//! `conv/claude/projects/…` and `conv/codex/…`.

use std::{collections::HashMap, fmt::Write as _, fs, path::Path};

use yard_domain::{
    Assignments, OrchestratorTerminalOutput, Projects, RuntimeInventory, WorkerCandidates,
};

use super::board::{self, Inputs, Item, LivePanes, Relevance};
use super::conversation::ConversationRoots;
use super::rules::{Column, Section};
use super::{HerdrAgents, agents_of};

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&fs::read(path).unwrap_or_else(|error| panic!("{path:?}: {error}")))
        .unwrap_or_else(|error| panic!("{path:?}: {error}"))
}

fn files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = fs::read_dir(dir).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        },
    );
    out.sort();
    out
}

/// "2 hr ago", like the web's `formatAgo`.
fn ago(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes < 1 {
        return "just now".to_owned();
    }
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} hr ago");
    }
    let days = hours / 24;
    if days < 60 {
        return format!("{days} day{} ago", if days == 1 { "" } else { "s" });
    }
    let months = days / 30;
    format!("{months} month{} ago", if months == 1 { "" } else { "s" })
}

/// The card's Status line as the web shows it.
pub(crate) fn status_line(item: &Item) -> String {
    if item.activity_known && item.column != Column::Doing && item.status.is_empty() {
        ago(item.age_ms)
    } else if item.activity_known && item.column != Column::Doing {
        format!("{} \u{b7} {}", item.status, ago(item.age_ms))
    } else {
        item.status.clone()
    }
}

fn place(item: &Item) -> &'static str {
    if item.helper == Some(super::plain::HELPER_IDLE) {
        return "Idle";
    }
    if item.helper.is_some() {
        return "Helper";
    }
    match (item.column, item.section) {
        (Column::NeedsYou, _) if item.need == Some(super::plain::NEED_REVIEW) => {
            "Needs you: Review"
        }
        (Column::NeedsYou, _) => "Needs you: Answer",
        (Column::Doing, _) => "Working now",
        (Column::Stale, Some(Section::LikelyDone)) => "Gone quiet: Quiet, probably done",
        (Column::Stale, _) if no_task(item) => "Gone quiet: Open, no task",
        (Column::Stale, _) => "Gone quiet: Stalled, not finished",
        (Column::Close, _) => "Finished: ready to close",
        (Column::Closed, _) => "Closed",
    }
}

/// A chat with no Yard task (untracked, or a worker with none), the web's
/// "Open, no task" group.
fn no_task(item: &Item) -> bool {
    item.chips
        .iter()
        .any(|chip| chip == "unassigned" || chip == "loose agent")
}

/// The web's per-project breakdown ("3 waiting on you, 1 working").
fn breakdown(cards: &[&&Item], project_id: &str) -> String {
    let open = cards
        .iter()
        .filter(|item| item.project_id.as_deref() == Some(project_id))
        .collect::<Vec<_>>();
    let count = |column: Column| open.iter().filter(|item| item.column == column).count();
    [
        (count(Column::NeedsYou), "waiting on you"),
        (count(Column::Doing), "working"),
        (count(Column::Stale), "quiet"),
        (count(Column::Close), "finished"),
    ]
    .into_iter()
    .filter(|(value, _)| *value > 0)
    .map(|(value, label)| format!("{value} {label}"))
    .collect::<Vec<_>>()
    .join(", ")
}

fn card(out: &mut String, item: &Item, badge: Option<&str>) {
    let mut flags = Vec::new();
    if let Some(badge) = badge {
        flags.push(badge);
    }
    if item.pinned {
        flags.push("kept");
    }
    if item.reviewed {
        flags.push("skipped");
    }
    if item.suggest_archive {
        flags.push("suggest archive");
    }
    if item.kind == super::rules::ItemKind::LooseAgent {
        flags.push("outside Yard");
    }
    let project = item.project_name.as_deref().unwrap_or("no project");
    let _ = writeln!(
        out,
        "- {}  [{project}; {}{}{}]",
        item.title,
        place(item),
        if flags.is_empty() { "" } else { "; " },
        flags.join(", ")
    );
    if !item.about.is_empty() {
        let _ = writeln!(out, "    About: {}", item.about);
    }
    let _ = writeln!(out, "    Status: {}", status_line(item));
    let _ = writeln!(out, "    Your move: {}", item.your_move);
    let _ = writeln!(out, "    (Details: title from {})", item.title_source);
}

/// One compact Older-work row, as the web lists it: title, age, Your move.
fn row(out: &mut String, item: &Item) {
    let age = if item.activity_known {
        format!(" \u{b7} {}", ago(item.age_ms))
    } else {
        String::new()
    };
    let _ = writeln!(out, "- {}{age} \u{b7} {}", item.title, item.your_move);
}

#[test]
#[allow(clippy::too_many_lines)]
#[ignore = "real-data check, run by hand against a private snapshot"]
fn print_the_board_for_a_real_snapshot() {
    let Ok(dir) = std::env::var("YARD_BOARD_REAL_SNAPSHOT") else {
        return;
    };
    let dir = Path::new(&dir);
    let api = dir.join("api");
    let now_ms = fs::read_to_string(api.join("now_ms"))
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    let projects = read::<Projects>(&api.join("projects.json")).projects;
    let workers = read::<WorkerCandidates>(&api.join("workers.json")).workers;
    let mut assignments = Vec::new();
    for path in files(&api.join("assignments")) {
        assignments.extend(read::<Assignments>(&path).assignments);
    }
    let mut reports = HashMap::new();
    for path in files(&api.join("reports")) {
        if let Ok(output) =
            serde_json::from_slice::<OrchestratorTerminalOutput>(&fs::read(&path).unwrap())
            && let Some(report) = output.status_report
        {
            reports.insert(output.project_id.clone(), report);
        }
    }
    let sessions: serde_json::Value = read(&api.join("sessions.json"));
    let mut live = LivePanes {
        running_sessions: sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|session| session["running"] == true)
            .filter_map(|session| session["name"].as_str().map(str::to_owned))
            .collect(),
        ..LivePanes::default()
    };
    let mut agents = HerdrAgents::default();
    for path in files(&api.join("inventories")) {
        agents_of(&read::<RuntimeInventory>(&path), &mut live, &mut agents);
    }
    let roots = ConversationRoots {
        claude_projects: Some(dir.join("conv/claude/projects")),
        codex_home: Some(dir.join("conv/codex")),
    };
    let mut wanted = agents
        .panes
        .values()
        .filter_map(|pane| Some((pane.provider.clone()?, pane.provider_session.clone()?)))
        .collect::<Vec<_>>();
    wanted.sort();
    wanted.dedup();
    let inputs = Inputs {
        now_ms,
        projects,
        assignments,
        workers,
        reports,
        live_panes: Some(live),
        pane_info: agents.panes,
        children: agents.children,
        loose_agents: agents.loose,
        conversations: roots.read(&wanted),
        ..Inputs::default()
    };
    let built = board::build(&inputs, None);
    let mut out = String::new();
    let cards = built
        .items
        .iter()
        .filter(|item| !item.rollup && item.column != Column::Closed)
        .collect::<Vec<_>>();
    let main = cards
        .iter()
        .filter(|item| board::on_main(item))
        .collect::<Vec<_>>();
    let ai_lines = built
        .items
        .iter()
        .filter(|item| item.ai.as_ref().is_some_and(|ai| !ai.about.is_empty()))
        .count();
    let _ = writeln!(
        out,
        "Work board v3 on a live snapshot: {} cards on the main board, Older work ({}), {} still to review, helpers ({}). Window: recent {} days, waiting {} days, suggest archive after {} days.",
        main.len(),
        built.older,
        built.to_review,
        built.helpers,
        built.window.recent_ms / board::DAY_MS,
        built.window.waiting_ms / board::DAY_MS,
        built.window.suggest_archive_ms / board::DAY_MS,
    );
    let _ = writeln!(
        out,
        "AI enrichment: {}",
        if ai_lines == 0 {
            "OFF in this check (no model call made); every line below is the deterministic text, built from the conversation's opening request and the agent's latest message".to_owned()
        } else {
            format!("on, {ai_lines} cards have AI lines")
        }
    );
    // To-dos a main-board card already says (review, answer, unblock) are
    // folded into that card as a "Do now" / "Soon" badge (as the web does);
    // the list keeps the rest.
    let on_board = main
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let folded = |todo: &super::todos::Todo| {
        matches!(
            todo.kind,
            super::todos::TodoKind::Review
                | super::todos::TodoKind::Reply
                | super::todos::TodoKind::Unblock
        ) && todo.sources.len() == 1
            && todo.sources[0].kind == "item"
            && on_board.contains(todo.sources[0].id.as_str())
    };
    let mut badges = HashMap::new();
    for todo in built.todos.iter().filter(|todo| folded(todo)) {
        let badge = if todo.urgency == super::todos::Urgency::Now {
            "Do now"
        } else {
            "Soon"
        };
        let entry = badges.entry(todo.sources[0].id.clone()).or_insert(badge);
        if badge == "Do now" {
            *entry = badge;
        }
    }
    let older = cards
        .iter()
        .filter(|item| item.helper.is_none() && item.relevance == Relevance::Older)
        .collect::<Vec<_>>();
    let untouched = older.iter().filter(|item| item.suggest_archive).count();
    for heading in [
        "Needs you: Answer",
        "Needs you: Review",
        "Working now",
        "Gone quiet: Stalled, not finished",
        "Gone quiet: Open, no task",
        "Gone quiet: Quiet, probably done",
        "Finished: ready to close",
    ] {
        let section = main
            .iter()
            .filter(|item| place(item) == heading)
            .collect::<Vec<_>>();
        if section.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n== MAIN BOARD: {heading} ({}) ==", section.len());
        for item in section {
            let why = match item.relevance {
                Relevance::Recent => "recent",
                Relevance::Waiting => "waiting on you",
                Relevance::Pinned => "kept",
                Relevance::Older => "older",
            };
            let _ = writeln!(out, "  (on the board because: {why})");
            card(&mut out, item, badges.get(&item.id).copied());
        }
    }
    let _ = writeln!(out, "\n== PROJECT HEADERS ({}) ==", built.projects.len());
    for project in &built.projects {
        let _ = writeln!(
            out,
            "- {} \u{b7} {}",
            project.name,
            breakdown(&main, &project.id)
        );
    }
    let mut archive = older
        .iter()
        .filter(|item| item.suggest_archive)
        .collect::<Vec<_>>();
    archive.sort_by_key(|item| std::cmp::Reverse(item.age_ms));
    let mut quiet = older
        .iter()
        .filter(|item| !item.suggest_archive)
        .collect::<Vec<_>>();
    quiet.sort_by_key(|item| item.age_ms);
    let days = built.window.suggest_archive_ms / board::DAY_MS;
    let weeks = if days % 7 == 0 {
        format!("{}+ week{}", days / 7, if days == 7 { "" } else { "s" })
    } else {
        format!("{days}+ days")
    };
    let _ = writeln!(
        out,
        "\n== OLDER WORK ({}) (collapsed; compact list) [button: Review older work ({})] ==",
        older.len(),
        built.to_review
    );
    let _ = writeln!(
        out,
        "  {untouched} untouched {weeks} (suggested for archive, reviewed first), {} quiet.",
        older.len() - untouched
    );
    let _ = writeln!(
        out,
        "-- Suggest archive ({}), oldest first --",
        archive.len()
    );
    for item in &archive {
        row(&mut out, item);
    }
    let _ = writeln!(out, "-- Quiet ({}), newest first --", quiet.len());
    for item in &quiet {
        row(&mut out, item);
    }
    let _ = writeln!(out, "\n== REVIEW DIALOG (one card at a time, full card) ==");
    for item in archive.iter().chain(quiet.iter()) {
        if item.suggest_archive {
            let _ = writeln!(
                out,
                "  (banner: Untouched {} days \u{2014} suggest archive. Primary: Archive)",
                item.age_ms / board::DAY_MS
            );
        }
        card(&mut out, item, None);
    }
    let helpers = cards
        .iter()
        .filter(|item| item.helper.is_some())
        .collect::<Vec<_>>();
    let _ = writeln!(
        out,
        "\n== HELPERS ({}) (hidden unless Show helpers) ==",
        built.helpers
    );
    let mut groups: Vec<(String, Vec<&Item>)> = Vec::new();
    for item in helpers {
        match groups.iter_mut().find(|(title, _)| *title == item.title) {
            Some((_, list)) => list.push(item),
            None => groups.push((item.title.clone(), vec![item])),
        }
    }
    for (title, list) in groups {
        if list.len() > 1 {
            let _ = writeln!(
                out,
                "- {title} ({}) [{}] (helper: {})",
                list.len(),
                place(list[0]),
                list[0].helper.unwrap_or_default(),
            );
        } else {
            let _ = writeln!(out, "  (helper: {})", list[0].helper.unwrap_or_default());
            card(&mut out, list[0], None);
        }
    }
    for row in &built.helper_rows {
        let _ = writeln!(
            out,
            "- Subagent: {} ({}, {}) under {}",
            row.title,
            row.status,
            ago(row.age_ms),
            row.parent_title
                .as_deref()
                .unwrap_or("an agent not on the board")
        );
    }
    let listed = built
        .todos
        .iter()
        .filter(|todo| !folded(todo))
        .collect::<Vec<_>>();
    let _ = writeln!(
        out,
        "\n== TO-DOS ({} listed; {} folded into their cards as a badge) ==",
        listed.len(),
        built.todos.len() - listed.len()
    );
    for todo in listed {
        let _ = writeln!(out, "- [{:?}] {}", todo.urgency, todo.text);
    }
    match std::env::var("YARD_BOARD_REAL_OUT") {
        Ok(path) => {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(out.as_bytes()).unwrap();
        }
        Err(_) => print!("{out}"),
    }
}
