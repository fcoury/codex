//! Helpers for rendering and navigating multi-agent state in the TUI.
//!
//! This module owns the shared presentation contracts for multi-agent history rows, `/agent` picker
//! entries, and the fast-switch keyboard shortcuts. Higher-level coordination, such as deciding
//! which thread becomes active or when a thread closes, stays in [`crate::app::App`].

use crate::history_cell::PlainHistoryCell;
use crate::render::line_utils::prefix_lines;
use crate::text_formatting::truncate_text;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::CollabAgentInteractionEndEvent;
use codex_protocol::protocol::CollabAgentRef;
use codex_protocol::protocol::CollabAgentSpawnEndEvent;
use codex_protocol::protocol::CollabAgentStatusEntry;
use codex_protocol::protocol::CollabCloseEndEvent;
use codex_protocol::protocol::CollabResumeBeginEvent;
use codex_protocol::protocol::CollabResumeEndEvent;
use codex_protocol::protocol::CollabWaitingBeginEvent;
use codex_protocol::protocol::CollabWaitingEndEvent;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
#[cfg(target_os = "macos")]
use crossterm::event::KeyEventKind;
#[cfg(target_os = "macos")]
use crossterm::event::KeyModifiers;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::collections::HashMap;
use std::collections::HashSet;

const COLLAB_PROMPT_PREVIEW_GRAPHEMES: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPickerThreadEntry {
    /// Human-friendly nickname shown in picker rows and footer labels.
    pub(crate) agent_nickname: Option<String>,
    /// Agent type shown in brackets when present, for example `worker`.
    pub(crate) agent_role: Option<String>,
    /// Whether the thread has emitted a close event and should render dimmed.
    pub(crate) is_closed: bool,
}

#[derive(Clone, Copy)]
struct AgentLabel<'a> {
    thread_id: Option<ThreadId>,
    nickname: Option<&'a str>,
    role: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnRequestSummary {
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffortConfig,
}

pub(crate) fn agent_picker_status_dot_spans(is_closed: bool) -> Vec<Span<'static>> {
    let dot = if is_closed {
        "•".into()
    } else {
        "•".green()
    };
    vec![dot, " ".into()]
}

pub(crate) fn format_agent_picker_item_name(
    agent_nickname: Option<&str>,
    agent_role: Option<&str>,
    is_primary: bool,
) -> String {
    if is_primary {
        return "Main [default]".to_string();
    }

    let agent_nickname = agent_nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty());
    let agent_role = agent_role.map(str::trim).filter(|role| !role.is_empty());
    match (agent_nickname, agent_role) {
        (Some(agent_nickname), Some(agent_role)) => format!("{agent_nickname} [{agent_role}]"),
        (Some(agent_nickname), None) => agent_nickname.to_string(),
        (None, Some(agent_role)) => format!("[{agent_role}]"),
        (None, None) => "Agent".to_string(),
    }
}

pub(crate) fn previous_agent_shortcut() -> crate::key_hint::KeyBinding {
    crate::key_hint::alt(KeyCode::Left)
}

pub(crate) fn next_agent_shortcut() -> crate::key_hint::KeyBinding {
    crate::key_hint::alt(KeyCode::Right)
}

/// Matches the canonical "previous agent" binding plus platform-specific fallbacks that keep agent
/// navigation working when enhanced key reporting is unavailable.
pub(crate) fn previous_agent_shortcut_matches(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    previous_agent_shortcut().is_press(key_event)
        || previous_agent_word_motion_fallback(key_event, allow_word_motion_fallback)
}

/// Matches the canonical "next agent" binding plus platform-specific fallbacks that keep agent
/// navigation working when enhanced key reporting is unavailable.
pub(crate) fn next_agent_shortcut_matches(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    next_agent_shortcut().is_press(key_event)
        || next_agent_word_motion_fallback(key_event, allow_word_motion_fallback)
}

#[cfg(target_os = "macos")]
fn previous_agent_word_motion_fallback(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    // Some terminals, especially on macOS, send Option+b/f as word-motion keys instead of
    // Option+arrow events unless enhanced keyboard reporting is enabled. Callers should only
    // enable this fallback when the composer is empty so draft editing retains the expected
    // word-wise motion behavior.
    allow_word_motion_fallback
        && matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Char('b'),
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        )
}

#[cfg(not(target_os = "macos"))]
fn previous_agent_word_motion_fallback(
    _key_event: KeyEvent,
    _allow_word_motion_fallback: bool,
) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn next_agent_word_motion_fallback(key_event: KeyEvent, allow_word_motion_fallback: bool) -> bool {
    // Some terminals, especially on macOS, send Option+b/f as word-motion keys instead of
    // Option+arrow events unless enhanced keyboard reporting is enabled. Callers should only
    // enable this fallback when the composer is empty so draft editing retains the expected
    // word-wise motion behavior.
    allow_word_motion_fallback
        && matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        )
}

#[cfg(not(target_os = "macos"))]
fn next_agent_word_motion_fallback(
    _key_event: KeyEvent,
    _allow_word_motion_fallback: bool,
) -> bool {
    false
}

pub(crate) fn spawn_end(
    ev: CollabAgentSpawnEndEvent,
    spawn_request: Option<&SpawnRequestSummary>,
) -> PlainHistoryCell {
    let CollabAgentSpawnEndEvent {
        call_id: _,
        sender_thread_id: _,
        new_thread_id,
        new_agent_nickname,
        new_agent_role,
        prompt,
        status: _,
        ..
    } = ev;

    let title = match new_thread_id {
        Some(thread_id) => title_with_agent(
            "Spawned",
            AgentLabel {
                thread_id: Some(thread_id),
                nickname: new_agent_nickname.as_deref(),
                role: new_agent_role.as_deref(),
            },
            spawn_request,
        ),
        None => title_text("Agent spawn failed"),
    };

    let mut details = Vec::new();
    if let Some(line) = prompt_line(&prompt) {
        details.push(line);
    }
    collab_event(title, details)
}

pub(crate) fn interaction_end(ev: CollabAgentInteractionEndEvent) -> PlainHistoryCell {
    let CollabAgentInteractionEndEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
        prompt,
        status: _,
    } = ev;

    let title = title_with_agent(
        "Sent input to",
        AgentLabel {
            thread_id: Some(receiver_thread_id),
            nickname: receiver_agent_nickname.as_deref(),
            role: receiver_agent_role.as_deref(),
        },
        /*spawn_request*/ None,
    );

    let mut details = Vec::new();
    if let Some(line) = prompt_line(&prompt) {
        details.push(line);
    }
    collab_event(title, details)
}

pub(crate) fn waiting_begin(ev: CollabWaitingBeginEvent) -> PlainHistoryCell {
    let CollabWaitingBeginEvent {
        sender_thread_id: _,
        receiver_thread_ids,
        receiver_agents,
        call_id: _,
    } = ev;
    let receiver_agents = merge_wait_receivers(&receiver_thread_ids, receiver_agents);

    let title = match receiver_agents.as_slice() {
        [receiver] => title_with_agent(
            "Waiting for",
            agent_label_from_ref(receiver),
            /*spawn_request*/ None,
        ),
        [] => title_text("Waiting for agents"),
        _ => title_text(format!("Waiting for {} agents", receiver_agents.len())),
    };

    let details = if receiver_agents.len() > 1 {
        receiver_agents
            .iter()
            .map(|receiver| agent_label_line(agent_label_from_ref(receiver)))
            .collect()
    } else {
        Vec::new()
    };

    collab_event(title, details)
}

pub(crate) fn waiting_end(ev: CollabWaitingEndEvent) -> PlainHistoryCell {
    let CollabWaitingEndEvent {
        call_id: _,
        sender_thread_id: _,
        agent_statuses,
        statuses,
    } = ev;
    let details = wait_complete_lines(&statuses, &agent_statuses);
    collab_event(title_text("Finished waiting"), details)
}

pub(crate) fn close_end(ev: CollabCloseEndEvent) -> PlainHistoryCell {
    let CollabCloseEndEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
        status: _,
    } = ev;

    collab_event(
        title_with_agent(
            "Closed",
            AgentLabel {
                thread_id: Some(receiver_thread_id),
                nickname: receiver_agent_nickname.as_deref(),
                role: receiver_agent_role.as_deref(),
            },
            /*spawn_request*/ None,
        ),
        Vec::new(),
    )
}

pub(crate) fn resume_begin(ev: CollabResumeBeginEvent) -> PlainHistoryCell {
    let CollabResumeBeginEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
    } = ev;

    collab_event(
        title_with_agent(
            "Resuming",
            AgentLabel {
                thread_id: Some(receiver_thread_id),
                nickname: receiver_agent_nickname.as_deref(),
                role: receiver_agent_role.as_deref(),
            },
            /*spawn_request*/ None,
        ),
        Vec::new(),
    )
}

pub(crate) fn resume_end(ev: CollabResumeEndEvent) -> PlainHistoryCell {
    let CollabResumeEndEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
        status,
    } = ev;

    collab_event(
        title_with_agent(
            "Resumed",
            AgentLabel {
                thread_id: Some(receiver_thread_id),
                nickname: receiver_agent_nickname.as_deref(),
                role: receiver_agent_role.as_deref(),
            },
            /*spawn_request*/ None,
        ),
        status_summary_lines(&status),
    )
}

fn collab_event(title: Line<'static>, details: Vec<Line<'static>>) -> PlainHistoryCell {
    let mut lines: Vec<Line<'static>> = vec![title];
    if !details.is_empty() {
        lines.extend(prefix_lines(details, "  └ ".dim(), "    ".into()));
    }
    PlainHistoryCell::new(lines)
}

fn title_text(title: impl Into<String>) -> Line<'static> {
    title_spans_line(vec![Span::from(title.into()).bold()])
}

fn title_with_agent(
    prefix: &str,
    agent: AgentLabel<'_>,
    spawn_request: Option<&SpawnRequestSummary>,
) -> Line<'static> {
    let mut spans = vec![Span::from(format!("{prefix} ")).bold()];
    spans.extend(agent_label_spans(agent));
    spans.extend(spawn_request_spans(spawn_request));
    title_spans_line(spans)
}

fn title_spans_line(mut spans: Vec<Span<'static>>) -> Line<'static> {
    let mut title = Vec::with_capacity(spans.len() + 1);
    title.push(Span::from("• ").dim());
    title.append(&mut spans);
    title.into()
}

fn agent_label_from_ref(agent: &CollabAgentRef) -> AgentLabel<'_> {
    AgentLabel {
        thread_id: Some(agent.thread_id),
        nickname: agent.agent_nickname.as_deref(),
        role: agent.agent_role.as_deref(),
    }
}

fn agent_label_line(agent: AgentLabel<'_>) -> Line<'static> {
    agent_label_spans(agent).into()
}

fn agent_label_spans(agent: AgentLabel<'_>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let nickname = agent
        .nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty());
    let role = agent.role.map(str::trim).filter(|role| !role.is_empty());

    if let Some(nickname) = nickname {
        spans.push(Span::from(nickname.to_string()).cyan().bold());
    } else if let Some(thread_id) = agent.thread_id {
        spans.push(Span::from(thread_id.to_string()).cyan());
    } else {
        spans.push(Span::from("agent").cyan());
    }

    if let Some(role) = role {
        spans.push(Span::from(" ").dim());
        spans.push(Span::from(format!("[{role}]")));
    }

    spans
}

fn spawn_request_spans(spawn_request: Option<&SpawnRequestSummary>) -> Vec<Span<'static>> {
    let Some(spawn_request) = spawn_request else {
        return Vec::new();
    };

    let model = spawn_request.model.trim();
    if model.is_empty() && spawn_request.reasoning_effort == ReasoningEffortConfig::default() {
        return Vec::new();
    }

    let details = if model.is_empty() {
        format!("({})", spawn_request.reasoning_effort)
    } else {
        format!("({model} {})", spawn_request.reasoning_effort)
    };

    vec![Span::from(" ").dim(), Span::from(details).magenta()]
}

fn prompt_line(prompt: &str) -> Option<Line<'static>> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(Line::from(Span::from(truncate_text(
            trimmed,
            COLLAB_PROMPT_PREVIEW_GRAPHEMES,
        ))))
    }
}

fn merge_wait_receivers(
    receiver_thread_ids: &[ThreadId],
    mut receiver_agents: Vec<CollabAgentRef>,
) -> Vec<CollabAgentRef> {
    if receiver_agents.is_empty() {
        return receiver_thread_ids
            .iter()
            .map(|thread_id| CollabAgentRef {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
            })
            .collect();
    }

    let mut seen = receiver_agents
        .iter()
        .map(|agent| agent.thread_id)
        .collect::<HashSet<_>>();
    for thread_id in receiver_thread_ids {
        if seen.insert(*thread_id) {
            receiver_agents.push(CollabAgentRef {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
            });
        }
    }
    receiver_agents
}

fn wait_complete_lines(
    statuses: &HashMap<ThreadId, AgentStatus>,
    agent_statuses: &[CollabAgentStatusEntry],
) -> Vec<Line<'static>> {
    if statuses.is_empty() && agent_statuses.is_empty() {
        return vec![Line::from(Span::from("No agents completed yet"))];
    }

    let entries = if agent_statuses.is_empty() {
        let mut entries = statuses
            .iter()
            .map(|(thread_id, status)| CollabAgentStatusEntry {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
                status: status.clone(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.thread_id.to_string().cmp(&right.thread_id.to_string()));
        entries
    } else {
        let mut entries = agent_statuses.to_vec();
        let seen = entries
            .iter()
            .map(|entry| entry.thread_id)
            .collect::<HashSet<_>>();
        let mut extras = statuses
            .iter()
            .filter(|(thread_id, _)| !seen.contains(thread_id))
            .map(|(thread_id, status)| CollabAgentStatusEntry {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
                status: status.clone(),
            })
            .collect::<Vec<_>>();
        extras.sort_by(|left, right| left.thread_id.to_string().cmp(&right.thread_id.to_string()));
        entries.extend(extras);
        entries
    };

    let mut lines = Vec::new();
    for entry in entries {
        let CollabAgentStatusEntry {
            thread_id,
            agent_nickname,
            agent_role,
            status,
        } = entry;
        let mut entry_lines = status_entry_lines(
            AgentLabel {
                thread_id: Some(thread_id),
                nickname: agent_nickname.as_deref(),
                role: agent_role.as_deref(),
            },
            &status,
        );
        if entry_lines.len() > 1 {
            entry_lines = prefix_lines(entry_lines, "".into(), "  ".into());
        }
        lines.extend(entry_lines);
    }
    lines
}

fn status_entry_lines(agent: AgentLabel<'_>, status: &AgentStatus) -> Vec<Line<'static>> {
    let mut summary_lines = status_summary_lines(status);
    let mut spans = agent_label_spans(agent);
    spans.push(Span::from(": ").dim());
    if let Some(first_summary_line) = summary_lines.first() {
        spans.extend(first_summary_line.spans.clone());
    }

    let mut lines = vec![Line::from(spans)];
    if !summary_lines.is_empty() {
        lines.extend(summary_lines.drain(1..));
    }
    lines
}

// Allow `.yellow()`
#[allow(clippy::disallowed_methods)]
fn status_summary_lines(status: &AgentStatus) -> Vec<Line<'static>> {
    match status {
        AgentStatus::PendingInit => vec![Line::from(vec![Span::from("Pending init").cyan()])],
        AgentStatus::Running => vec![Line::from(vec![Span::from("Running").cyan().bold()])],
        AgentStatus::Interrupted => vec![Line::from(vec![Span::from("Interrupted").yellow()])],
        AgentStatus::Completed(message) => {
            let mut lines = vec![Line::from(vec![Span::from("Completed").green()])];
            if let Some(message) = message.as_ref() {
                lines.extend(status_message_lines(message));
            }
            lines
        }
        AgentStatus::Errored(error) => {
            let mut lines = vec![Line::from(vec![Span::from("Error").red()])];
            lines.extend(status_message_lines(error));
            lines
        }
        AgentStatus::Shutdown => vec![Line::from("Shutdown")],
        AgentStatus::NotFound => vec![Line::from(vec![Span::from("Not found").red()])],
    }
}

fn status_message_lines(message: &str) -> Vec<Line<'static>> {
    let trimmed = message.trim_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Vec::new();
    }

    trimmed
        .split('\n')
        .map(|line| Line::from(line.trim_end_matches('\r').to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::HistoryCell;
    #[cfg(target_os = "macos")]
    use crossterm::event::KeyEvent;
    #[cfg(target_os = "macos")]
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::style::Modifier;
    use ratatui::text::Text;
    use ratatui::widgets::Paragraph;
    use ratatui::widgets::WidgetRef;
    use ratatui::widgets::Wrap;

    struct Issue15001Scenario<'a> {
        name: &'a str,
        message: &'a str,
        expected_substrings: &'a [&'a str],
    }

    #[test]
    fn collab_events_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");
        let bob_id = ThreadId::from_string("00000000-0000-0000-0000-000000000003")
            .expect("valid bob thread id");

        let spawn = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn".to_string(),
                sender_thread_id,
                new_thread_id: Some(robie_id),
                new_agent_nickname: Some("Robie".to_string()),
                new_agent_role: Some("explorer".to_string()),
                prompt: "Compute 11! and reply with just the integer result.".to_string(),
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                status: AgentStatus::PendingInit,
            },
            Some(&SpawnRequestSummary {
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
            }),
        );

        let send = interaction_end(CollabAgentInteractionEndEvent {
            call_id: "call-send".to_string(),
            sender_thread_id,
            receiver_thread_id: robie_id,
            receiver_agent_nickname: Some("Robie".to_string()),
            receiver_agent_role: Some("explorer".to_string()),
            prompt: "Please continue and return the answer only.".to_string(),
            status: AgentStatus::Running,
        });

        let waiting = waiting_begin(CollabWaitingBeginEvent {
            sender_thread_id,
            receiver_thread_ids: vec![robie_id],
            receiver_agents: vec![CollabAgentRef {
                thread_id: robie_id,
                agent_nickname: Some("Robie".to_string()),
                agent_role: Some("explorer".to_string()),
            }],
            call_id: "call-wait".to_string(),
        });

        let mut statuses = HashMap::new();
        statuses.insert(
            robie_id,
            AgentStatus::Completed(Some("39916800".to_string())),
        );
        statuses.insert(bob_id, AgentStatus::Errored("tool timeout".to_string()));
        let finished = waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: "call-wait".to_string(),
            agent_statuses: vec![
                CollabAgentStatusEntry {
                    thread_id: robie_id,
                    agent_nickname: Some("Robie".to_string()),
                    agent_role: Some("explorer".to_string()),
                    status: AgentStatus::Completed(Some("39916800".to_string())),
                },
                CollabAgentStatusEntry {
                    thread_id: bob_id,
                    agent_nickname: Some("Bob".to_string()),
                    agent_role: Some("worker".to_string()),
                    status: AgentStatus::Errored("tool timeout".to_string()),
                },
            ],
            statuses,
        });

        let close = close_end(CollabCloseEndEvent {
            call_id: "call-close".to_string(),
            sender_thread_id,
            receiver_thread_id: robie_id,
            receiver_agent_nickname: Some("Robie".to_string()),
            receiver_agent_role: Some("explorer".to_string()),
            status: AgentStatus::Completed(Some("39916800".to_string())),
        });

        let snapshot = [spawn, send, waiting, finished, close]
            .iter()
            .map(cell_to_text)
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_snapshot!("collab_agent_transcript", snapshot);
    }

    #[test]
    fn waiting_end_preserves_multiline_completed_messages() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let receiver_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid receiver thread id");
        let message = "Вношу две узкие правки в tooling docs: уточняю исключение для `Architect initial draft` и добавляю owner-facing инструкцию по `racex_plan_bundle`.\nMirror-патч не прошёл через `apply_patch`; tooling-файл отдельно дожму сейчас, а для sibling mirror, если понадобится, пойду через явное разрешение на один безопасный sync-патч.";
        let mut statuses = HashMap::new();
        statuses.insert(
            receiver_thread_id,
            AgentStatus::Completed(Some(message.to_string())),
        );
        let cell = waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: "call-wait".to_string(),
            agent_statuses: vec![CollabAgentStatusEntry {
                thread_id: receiver_thread_id,
                agent_nickname: Some("Atlas".to_string()),
                agent_role: Some("worker".to_string()),
                status: AgentStatus::Completed(Some(message.to_string())),
            }],
            statuses,
        });

        assert_snapshot!(cell_to_text(&cell));
    }

    #[test]
    fn waiting_end_preserves_negation_and_paths_in_completed_messages() {
        let scenario = issue_15001_scenarios()
            .into_iter()
            .find(|scenario| scenario.name == "negation_and_paths")
            .expect("scenario should exist");
        let rendered = cell_to_text(&waiting_end_with_message(scenario.message));
        for expected_substring in scenario.expected_substrings {
            assert!(
                rendered.contains(expected_substring),
                "missing expected token `{expected_substring}` in rendered output:\n{rendered}"
            );
        }

        assert_snapshot!(rendered);
    }

    #[test]
    fn waiting_end_issue_15001_scenarios_preserve_expected_tokens() {
        for scenario in issue_15001_scenarios() {
            let rendered = cell_to_text(&waiting_end_with_message(scenario.message));
            for expected_substring in scenario.expected_substrings {
                assert!(
                    rendered.contains(expected_substring),
                    "scenario `{}` dropped expected token `{expected_substring}` in rendered output:\n{rendered}",
                    scenario.name
                );
            }
            assert_snapshot!(scenario.name, rendered);
        }
    }

    #[test]
    fn waiting_end_preserves_multiple_agent_blocks() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let atlas_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid atlas thread id");
        let bob_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000003")
            .expect("valid bob thread id");
        let atlas_message = issue_15001_scenarios()[0].message;
        let bob_message =
            "Need a second pass on the tmux capture.\nThe raw rollout is still correct.";
        let cell = waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: "call-wait".to_string(),
            agent_statuses: vec![
                CollabAgentStatusEntry {
                    thread_id: atlas_thread_id,
                    agent_nickname: Some("Atlas".to_string()),
                    agent_role: Some("worker".to_string()),
                    status: AgentStatus::Completed(Some(atlas_message.to_string())),
                },
                CollabAgentStatusEntry {
                    thread_id: bob_thread_id,
                    agent_nickname: Some("Bob".to_string()),
                    agent_role: Some("explorer".to_string()),
                    status: AgentStatus::Completed(Some(bob_message.to_string())),
                },
            ],
            statuses: HashMap::from([
                (
                    atlas_thread_id,
                    AgentStatus::Completed(Some(atlas_message.to_string())),
                ),
                (
                    bob_thread_id,
                    AgentStatus::Completed(Some(bob_message.to_string())),
                ),
            ]),
        });

        let rendered = cell_to_text(&cell);
        assert!(rendered.contains("Atlas [worker]: Completed"));
        assert!(rendered.contains("Bob [explorer]: Completed"));
        assert_snapshot!(rendered);
    }

    #[test]
    fn waiting_end_issue_15001_width_matrix_snapshot() {
        let cell = waiting_end_with_message(issue_15001_scenarios()[0].message);
        snapshot_rendered_cell("waiting_end_issue_15001_width_80", &cell, 80, 10);
        snapshot_rendered_cell("waiting_end_issue_15001_width_120", &cell, 120, 10);
        snapshot_rendered_cell("waiting_end_issue_15001_width_160", &cell, 160, 10);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn agent_shortcut_matches_option_arrow_word_motion_fallbacks_only_when_allowed() {
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            false,
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
            false,
        ));
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            true,
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            true,
        ));
        assert!(!previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            false,
        ));
        assert!(!next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            false,
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn agent_shortcut_matches_option_arrows_only() {
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Left, crossterm::event::KeyModifiers::ALT,),
            false
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Right, crossterm::event::KeyModifiers::ALT,),
            false
        ));
        assert!(!previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), crossterm::event::KeyModifiers::ALT,),
            false
        ));
        assert!(!next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), crossterm::event::KeyModifiers::ALT,),
            false
        ));
    }

    #[test]
    fn title_styles_nickname_and_role() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");
        let cell = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn".to_string(),
                sender_thread_id,
                new_thread_id: Some(robie_id),
                new_agent_nickname: Some("Robie".to_string()),
                new_agent_role: Some("explorer".to_string()),
                prompt: String::new(),
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                status: AgentStatus::PendingInit,
            },
            Some(&SpawnRequestSummary {
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
            }),
        );

        let lines = cell.display_lines(200);
        let title = &lines[0];
        assert_eq!(title.spans[2].content.as_ref(), "Robie");
        assert_eq!(title.spans[2].style.fg, Some(Color::Cyan));
        assert!(title.spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(title.spans[4].content.as_ref(), "[explorer]");
        assert_eq!(title.spans[4].style.fg, None);
        assert!(!title.spans[4].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(title.spans[6].content.as_ref(), "(gpt-5 high)");
        assert_eq!(title.spans[6].style.fg, Some(Color::Magenta));
    }

    #[test]
    fn collab_resume_interrupted_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");

        let cell = resume_end(CollabResumeEndEvent {
            call_id: "call-resume".to_string(),
            sender_thread_id,
            receiver_thread_id: robie_id,
            receiver_agent_nickname: Some("Robie".to_string()),
            receiver_agent_role: Some("explorer".to_string()),
            status: AgentStatus::Interrupted,
        });

        assert_snapshot!("collab_resume_interrupted", cell_to_text(&cell));
    }

    fn cell_to_text(cell: &PlainHistoryCell) -> String {
        cell.display_lines(200)
            .iter()
            .map(line_to_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn waiting_end_with_message(message: &str) -> PlainHistoryCell {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let receiver_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid receiver thread id");
        waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: "call-wait".to_string(),
            agent_statuses: vec![CollabAgentStatusEntry {
                thread_id: receiver_thread_id,
                agent_nickname: Some("Atlas".to_string()),
                agent_role: Some("worker".to_string()),
                status: AgentStatus::Completed(Some(message.to_string())),
            }],
            statuses: HashMap::from([(
                receiver_thread_id,
                AgentStatus::Completed(Some(message.to_string())),
            )]),
        })
    }

    fn issue_15001_scenarios() -> Vec<Issue15001Scenario<'static>> {
        vec![
            Issue15001Scenario {
                name: "multiline_cyrillic_with_backticks",
                message: "Вношу две узкие правки в tooling docs: уточняю исключение для `Architect initial draft` и добавляю owner-facing инструкцию по `racex_plan_bundle`.\nMirror-патч не прошёл через `apply_patch`; tooling-файл отдельно дожму сейчас, а для sibling mirror, если понадобится, пойду через явное разрешение на один безопасный sync-патч.",
                expected_substrings: &[
                    "Architect initial draft",
                    "racex_plan_bundle",
                    "apply_patch",
                ],
            },
            Issue15001Scenario {
                name: "negation_and_paths",
                message: "Да, это уже реальный blocker.\nЛог menubar app теперь такой:\nauth pool at `/Users/palvaleri/codex-session-router/var/auth` contains no valid profiles",
                expected_substrings: &[
                    "/Users/palvaleri/codex-session-router/var/auth",
                    "contains no valid profiles",
                ],
            },
            Issue15001Scenario {
                name: "ascii_control",
                message: "Replaying the preview path with ASCII control text.\nKeep `S2` and `N/A` intact while preserving `/tmp/session-router/auth`.",
                expected_substrings: &["S2", "N/A", "/tmp/session-router/auth"],
            },
        ]
    }

    fn snapshot_rendered_cell(name: &str, cell: &PlainHistoryCell, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        let lines = cell.display_lines(width);
        terminal
            .draw(|frame| {
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .render_ref(frame.area(), frame.buffer_mut())
            })
            .expect("draw");
        assert_snapshot!(name, terminal.backend());
    }

    fn line_to_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }
}
