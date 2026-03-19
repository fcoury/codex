use crate::history_cell::HistoryCell;
use crate::history_cell::{self};
use crate::render::line_utils::prefix_lines;
use crate::style::proposed_plan_style;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use super::StreamState;

/// Controller that manages newline-gated streaming, header emission, and
/// commit animation across streams.
pub(crate) struct StreamController {
    state: StreamState,
    finishing_after_drain: bool,
    header_emitted: bool,
}

impl StreamController {
    /// Create a controller whose markdown renderer shortens local file links relative to `cwd`.
    ///
    /// The controller snapshots the path into stream state so later commit ticks and finalization
    /// render against the same session cwd that was active when streaming started.
    pub(crate) fn new(width: Option<usize>, cwd: &Path) -> Self {
        Self {
            state: StreamState::new(width, cwd),
            finishing_after_drain: false,
            header_emitted: false,
        }
    }

    /// Push a delta; if it contains a newline, commit completed lines and start animation.
    pub(crate) fn push(&mut self, delta: &str) -> bool {
        let state = &mut self.state;
        if !delta.is_empty() {
            state.has_seen_delta = true;
        }
        state.collector.push_delta(delta);
        if delta.contains('\n') {
            let newly_completed = state.collector.commit_complete_lines();
            if !newly_completed.is_empty() {
                state.enqueue(newly_completed);
                return true;
            }
        }
        false
    }

    /// Finalize the active stream. Drain and emit now.
    pub(crate) fn finalize(&mut self) -> Option<Box<dyn HistoryCell>> {
        // Finalize collector first.
        let remaining = {
            let state = &mut self.state;
            state.collector.finalize_and_drain()
        };
        // Collect all output first to avoid emitting headers when there is no content.
        let mut out_lines = Vec::new();
        {
            let state = &mut self.state;
            if !remaining.is_empty() {
                state.enqueue(remaining);
            }
            let step = state.drain_all();
            out_lines.extend(step);
        }

        // Cleanup
        self.state.clear();
        self.finishing_after_drain = false;
        self.emit(out_lines)
    }

    /// Step animation: commit at most one queued line and handle end-of-drain cleanup.
    pub(crate) fn on_commit_tick(&mut self) -> (Option<Box<dyn HistoryCell>>, bool) {
        let step = self.state.step();
        (self.emit(step), self.state.is_idle())
    }

    /// Step animation: commit at most `max_lines` queued lines.
    ///
    /// This is intended for adaptive catch-up drains. Callers should keep `max_lines` bounded; a
    /// very large value can collapse perceived animation into a single jump.
    pub(crate) fn on_commit_tick_batch(
        &mut self,
        max_lines: usize,
    ) -> (Option<Box<dyn HistoryCell>>, bool) {
        let step = self.state.drain_n(max_lines.max(1));
        (self.emit(step), self.state.is_idle())
    }

    /// Returns the current number of queued lines waiting to be displayed.
    pub(crate) fn queued_lines(&self) -> usize {
        self.state.queued_len()
    }

    /// Returns the age of the oldest queued line.
    pub(crate) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.state.oldest_queued_age(now)
    }

    fn emit(&mut self, lines: Vec<Line<'static>>) -> Option<Box<dyn HistoryCell>> {
        if lines.is_empty() {
            return None;
        }
        Some(Box::new(history_cell::AgentMessageCell::new(lines, {
            let header_emitted = self.header_emitted;
            self.header_emitted = true;
            !header_emitted
        })))
    }
}

/// Controller that streams proposed plan markdown into a styled plan block.
pub(crate) struct PlanStreamController {
    state: StreamState,
    header_emitted: bool,
    top_padding_emitted: bool,
}

impl PlanStreamController {
    /// Create a plan-stream controller whose markdown renderer shortens local file links relative
    /// to `cwd`.
    ///
    /// The controller snapshots the path into stream state so later commit ticks and finalization
    /// render against the same session cwd that was active when streaming started.
    pub(crate) fn new(width: Option<usize>, cwd: &Path) -> Self {
        Self {
            state: StreamState::new(width, cwd),
            header_emitted: false,
            top_padding_emitted: false,
        }
    }

    /// Push a delta; if it contains a newline, commit completed lines and start animation.
    pub(crate) fn push(&mut self, delta: &str) -> bool {
        let state = &mut self.state;
        if !delta.is_empty() {
            state.has_seen_delta = true;
        }
        state.collector.push_delta(delta);
        if delta.contains('\n') {
            let newly_completed = state.collector.commit_complete_lines();
            if !newly_completed.is_empty() {
                state.enqueue(newly_completed);
                return true;
            }
        }
        false
    }

    /// Finalize the active stream. Drain and emit now.
    pub(crate) fn finalize(&mut self) -> Option<Box<dyn HistoryCell>> {
        let remaining = {
            let state = &mut self.state;
            state.collector.finalize_and_drain()
        };
        let mut out_lines = Vec::new();
        {
            let state = &mut self.state;
            if !remaining.is_empty() {
                state.enqueue(remaining);
            }
            let step = state.drain_all();
            out_lines.extend(step);
        }

        self.state.clear();
        self.emit(out_lines, /*include_bottom_padding*/ true)
    }

    /// Step animation: commit at most one queued line and handle end-of-drain cleanup.
    pub(crate) fn on_commit_tick(&mut self) -> (Option<Box<dyn HistoryCell>>, bool) {
        let step = self.state.step();
        (
            self.emit(step, /*include_bottom_padding*/ false),
            self.state.is_idle(),
        )
    }

    /// Step animation: commit at most `max_lines` queued lines.
    ///
    /// This is intended for adaptive catch-up drains. Callers should keep `max_lines` bounded; a
    /// very large value can collapse perceived animation into a single jump.
    pub(crate) fn on_commit_tick_batch(
        &mut self,
        max_lines: usize,
    ) -> (Option<Box<dyn HistoryCell>>, bool) {
        let step = self.state.drain_n(max_lines.max(1));
        (
            self.emit(step, /*include_bottom_padding*/ false),
            self.state.is_idle(),
        )
    }

    /// Returns the current number of queued plan lines waiting to be displayed.
    pub(crate) fn queued_lines(&self) -> usize {
        self.state.queued_len()
    }

    /// Returns the age of the oldest queued plan line.
    pub(crate) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.state.oldest_queued_age(now)
    }

    fn emit(
        &mut self,
        lines: Vec<Line<'static>>,
        include_bottom_padding: bool,
    ) -> Option<Box<dyn HistoryCell>> {
        if lines.is_empty() && !include_bottom_padding {
            return None;
        }

        let mut out_lines: Vec<Line<'static>> = Vec::new();
        let is_stream_continuation = self.header_emitted;
        if !self.header_emitted {
            out_lines.push(vec!["• ".dim(), "Proposed Plan".bold()].into());
            out_lines.push(Line::from(" "));
            self.header_emitted = true;
        }

        let mut plan_lines: Vec<Line<'static>> = Vec::new();
        if !self.top_padding_emitted {
            plan_lines.push(Line::from(" "));
            self.top_padding_emitted = true;
        }
        plan_lines.extend(lines);
        if include_bottom_padding {
            plan_lines.push(Line::from(" "));
        }

        let plan_style = proposed_plan_style();
        let plan_lines = prefix_lines(plan_lines, "  ".into(), "  ".into())
            .into_iter()
            .map(|line| line.style(plan_style))
            .collect::<Vec<_>>();
        out_lines.extend(plan_lines);

        Some(Box::new(history_cell::new_proposed_plan_stream(
            out_lines,
            is_stream_continuation,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::path::PathBuf;

    fn test_cwd() -> PathBuf {
        // These tests only need a stable absolute cwd; using temp_dir() avoids baking Unix- or
        // Windows-specific root semantics into the fixtures.
        std::env::temp_dir()
    }

    fn lines_to_plain_strings(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect()
    }

    #[derive(Debug)]
    struct ControllerTrace {
        display_width: usize,
        deltas: Vec<String>,
        transcript: Vec<String>,
        visible_rows: Vec<String>,
        full_render: Vec<String>,
    }

    impl fmt::Display for ControllerTrace {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            writeln!(f, "display_width: {}", self.display_width)?;
            writeln!(f, "deltas:")?;
            for (idx, delta) in self.deltas.iter().enumerate() {
                writeln!(f, "  [{idx}] {:?}", delta)?;
            }
            writeln!(f, "transcript: {:?}", self.transcript)?;
            writeln!(f, "visible_rows: {:?}", self.visible_rows)?;
            writeln!(f, "full_render: {:?}", self.full_render)
        }
    }

    fn render_markdown_to_plain_strings(source: &str, width: Option<usize>) -> Vec<String> {
        let mut rendered: Vec<ratatui::text::Line<'static>> = Vec::new();
        let test_cwd = test_cwd();
        crate::markdown::append_markdown(source, width, Some(test_cwd.as_path()), &mut rendered);
        lines_to_plain_strings(&rendered)
    }

    fn strip_agent_prefix(line: String) -> String {
        line.chars().skip(2).collect()
    }

    fn collect_controller_trace(deltas: &[&str], display_width: usize) -> ControllerTrace {
        let collector_width = display_width.saturating_sub(2);
        let mut ctrl = StreamController::new(Some(collector_width), &test_cwd());
        let mut transcript = Vec::new();
        let mut visible_rows = Vec::new();

        for delta in deltas {
            ctrl.push(delta);
            while let (Some(cell), idle) = ctrl.on_commit_tick() {
                transcript.extend(lines_to_plain_strings(&cell.transcript_lines(u16::MAX)));
                visible_rows.extend(
                    lines_to_plain_strings(&cell.display_lines(display_width as u16))
                        .into_iter()
                        .map(strip_agent_prefix),
                );
                if idle {
                    break;
                }
            }
        }

        if let Some(cell) = ctrl.finalize() {
            transcript.extend(lines_to_plain_strings(&cell.transcript_lines(u16::MAX)));
            visible_rows.extend(
                lines_to_plain_strings(&cell.display_lines(display_width as u16))
                    .into_iter()
                    .map(strip_agent_prefix),
            );
        }

        let full_source: String = deltas.iter().copied().collect();
        let full_render = render_markdown_to_plain_strings(&full_source, Some(collector_width));

        ControllerTrace {
            display_width,
            deltas: deltas.iter().map(|delta| (*delta).to_string()).collect(),
            transcript,
            visible_rows,
            full_render,
        }
    }

    fn assert_controller_matches_full(label: &str, deltas: &[&str], display_width: usize) {
        let trace = collect_controller_trace(deltas, display_width);
        assert_eq!(
            trace.transcript, trace.full_render,
            "{label} diverged at transcript layer\n{trace}"
        );
        assert_eq!(
            trace.visible_rows, trace.full_render,
            "{label} diverged at visible row layer\n{trace}"
        );
    }

    #[tokio::test]
    async fn controller_loose_vs_tight_with_commit_ticks_matches_full() {
        let mut ctrl = StreamController::new(None, &test_cwd());
        let mut lines = Vec::new();

        // Exact deltas from the session log (section: Loose vs. tight list items)
        let deltas = vec![
            "\n\n",
            "Loose",
            " vs",
            ".",
            " tight",
            " list",
            " items",
            ":\n",
            "1",
            ".",
            " Tight",
            " item",
            "\n",
            "2",
            ".",
            " Another",
            " tight",
            " item",
            "\n\n",
            "1",
            ".",
            " Loose",
            " item",
            " with",
            " its",
            " own",
            " paragraph",
            ".\n\n",
            "  ",
            " This",
            " paragraph",
            " belongs",
            " to",
            " the",
            " same",
            " list",
            " item",
            ".\n\n",
            "2",
            ".",
            " Second",
            " loose",
            " item",
            " with",
            " a",
            " nested",
            " list",
            " after",
            " a",
            " blank",
            " line",
            ".\n\n",
            "  ",
            " -",
            " Nested",
            " bullet",
            " under",
            " a",
            " loose",
            " item",
            "\n",
            "  ",
            " -",
            " Another",
            " nested",
            " bullet",
            "\n\n",
        ];

        // Simulate streaming with a commit tick attempt after each delta.
        for d in deltas.iter() {
            ctrl.push(d);
            while let (Some(cell), idle) = ctrl.on_commit_tick() {
                lines.extend(cell.transcript_lines(u16::MAX));
                if idle {
                    break;
                }
            }
        }
        // Finalize and flush remaining lines now.
        if let Some(cell) = ctrl.finalize() {
            lines.extend(cell.transcript_lines(u16::MAX));
        }

        let streamed: Vec<_> = lines_to_plain_strings(&lines)
            .into_iter()
            // skip • and 2-space indentation
            .map(|s| s.chars().skip(2).collect::<String>())
            .collect();

        // Full render of the same source
        let source: String = deltas.iter().copied().collect();
        let mut rendered: Vec<ratatui::text::Line<'static>> = Vec::new();
        let test_cwd = test_cwd();
        crate::markdown::append_markdown(&source, None, Some(test_cwd.as_path()), &mut rendered);
        let rendered_strs = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, rendered_strs);

        // Also assert exact expected plain strings for clarity.
        let expected = vec![
            "Loose vs. tight list items:".to_string(),
            "".to_string(),
            "1. Tight item".to_string(),
            "2. Another tight item".to_string(),
            "3. Loose item with its own paragraph.".to_string(),
            "".to_string(),
            "   This paragraph belongs to the same list item.".to_string(),
            "4. Second loose item with a nested list after a blank line.".to_string(),
            "    - Nested bullet under a loose item".to_string(),
            "    - Another nested bullet".to_string(),
        ];
        assert_eq!(
            streamed, expected,
            "expected exact rendered lines for loose/tight section"
        );
    }

    #[tokio::test]
    async fn controller_inline_code_completion_rewrites_prior_line_matches_full() {
        let deltas = ["Проверяю `S2` vs `\n", "N/A`\n"];

        for display_width in [26usize, 42, 82] {
            assert_controller_matches_full(
                "stream controller should not preserve stale pre-closure inline-code output",
                &deltas,
                display_width,
            );
        }
    }

    #[tokio::test]
    async fn controller_issue_15001_repro_b_matches_full() {
        let deltas = [
            "Evidence собран; перехожу к reviewer artifact. Открою один-два свежих backend review-файла, чтобы сохранить repo-local формат и правильно зафиксировать `S2` vs `\n",
            "N/A` для visual review.\n",
        ];

        for display_width in [42usize, 50, 62, 74, 82] {
            assert_controller_matches_full(
                "stream controller should match one-shot render for issue-15001 repro B",
                &deltas,
                display_width,
            );
        }
    }
}
