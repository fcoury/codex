use crate::history_cell::HistoryCell;
use crate::history_cell::{self};
use crate::render::line_utils::prefix_lines;
use crate::style::proposed_plan_style;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use std::time::Duration;
use std::time::Instant;

use super::StreamState;

/// Controller for streaming agent message content with table-aware holdback.
///
/// Manages a two-region model of rendered output:
///
/// - **Stable region** (`[0..enqueued_stable_len]`): lines that have been
///   enqueued for commit-animation drain into scrollback. Once emitted
///   (`emitted_stable_len`), these lines live in the transcript as
///   `AgentMessageCell`s.
///
/// - **Tail region** (`[enqueued_stable_len..]`): lines that are still
///   mutable because they may be rewritten (e.g. a table whose column widths
///   change as new rows arrive). Displayed as a `StreamingAgentTailCell` in
///   the active cell.
///
/// When `table_holdback_state()` detects an in-progress table, the entire
/// rendered buffer becomes tail (tail_budget = rendered_lines.len()), preventing
/// partial-table commits. When no table is detected, the tail budget is zero
/// and all rendered lines flow into the stable queue immediately.
///
/// On terminal resize, `set_width()` re-renders from `raw_source`, remaps
/// `emitted_stable_len` to the new layout via `source_bytes_for_rendered_count`,
/// and rebuilds the stable queue.
pub(crate) struct StreamController {
    state: StreamState,
    /// Current rendering width (columns available for markdown content).
    width: Option<usize>,
    /// Accumulated raw markdown source for the current stream (newline-terminated
    /// chunks from `commit_complete_source()`).
    raw_source: String,
    /// Full re-render of `raw_source` at `width`. Rebuilt on every committed delta.
    rendered_lines: Vec<Line<'static>>,
    /// Number of lines from `rendered_lines[0..]` that have been enqueued into
    /// the commit-animation queue (but may not yet be emitted to scrollback).
    enqueued_stable_len: usize,
    /// Number of lines that have actually been emitted to scrollback via
    /// `on_commit_tick` / `on_commit_tick_batch`. Always <= `enqueued_stable_len`.
    emitted_stable_len: usize,
    header_emitted: bool,
}

impl StreamController {
    pub(crate) fn new(width: Option<usize>) -> Self {
        Self {
            state: StreamState::new(width),
            width,
            raw_source: String::new(),
            rendered_lines: Vec::new(),
            enqueued_stable_len: 0,
            emitted_stable_len: 0,
            header_emitted: false,
        }
    }

    /// Push a delta; if it contains a newline, commit completed lines and start animation.
    pub(crate) fn push(&mut self, delta: &str) -> bool {
        if !delta.is_empty() {
            self.state.has_seen_delta = true;
        }
        self.state.collector.push_delta(delta);

        let mut enqueued = false;
        if delta.contains('\n')
            && let Some(committed_source) = self.state.collector.commit_complete_source()
        {
            self.raw_source.push_str(&committed_source);
            self.recompute_streaming_render();
            enqueued = self.sync_stable_queue();
        }
        enqueued
    }

    /// Finalize the active stream. Returns the final cell (if any remaining
    /// lines) and the raw markdown source for consolidation.
    pub(crate) fn finalize(&mut self) -> (Option<Box<dyn HistoryCell>>, Option<String>) {
        let remainder_source = self.state.collector.finalize_and_drain_source();
        if !remainder_source.is_empty() {
            self.raw_source.push_str(&remainder_source);
        }
        if self.raw_source.is_empty() {
            self.reset_stream_state();
            return (None, None);
        }

        // Capture the source before reset clears it.
        let source = self.raw_source.clone();

        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&self.raw_source, self.width, &mut rendered);
        let out_lines = if self.emitted_stable_len >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.emitted_stable_len..].to_vec()
        };

        let out = self.emit(out_lines);
        self.reset_stream_state();
        (out, Some(source))
    }

    /// Step animation: commit at most one queued line and handle end-of-drain cleanup.
    pub(crate) fn on_commit_tick(&mut self) -> (Option<Box<dyn HistoryCell>>, bool) {
        let step = self.state.step();
        self.emitted_stable_len += step.len();
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
        self.emitted_stable_len += step.len();
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

    /// Returns the current mutable tail to render in the active cell.
    pub(crate) fn current_tail_lines(&self) -> Vec<Line<'static>> {
        let start = self.enqueued_stable_len.min(self.rendered_lines.len());
        self.rendered_lines[start..].to_vec()
    }

    /// Returns whether the mutable tail should render the leading assistant marker.
    pub(crate) fn tail_starts_stream(&self) -> bool {
        !self.header_emitted && self.enqueued_stable_len == 0
    }

    pub(crate) fn has_live_tail(&self) -> bool {
        !self.current_tail_lines().is_empty()
    }

    /// Update rendering width and rebuild queued stable lines to match the new layout.
    pub(crate) fn set_width(&mut self, width: Option<usize>) {
        if self.width == width {
            return;
        }
        let old_width = self.width;
        self.width = width;
        self.state.collector.set_width(width);
        if self.raw_source.is_empty() {
            return;
        }

        // Recalculate emitted_stable_len for the new width so we don't
        // re-queue lines that were already emitted at the old width.
        if self.emitted_stable_len > 0 {
            let emitted_bytes = source_bytes_for_rendered_count(
                &self.raw_source,
                old_width,
                self.emitted_stable_len,
            );
            let mut emitted_at_new = Vec::new();
            crate::markdown::append_markdown_agent(
                &self.raw_source[..emitted_bytes],
                self.width,
                &mut emitted_at_new,
            );
            self.emitted_stable_len = emitted_at_new.len();
        }

        self.recompute_streaming_render();
        self.rebuild_stable_queue_from_render();
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

    /// Re-render the full `raw_source` at the current `width`, replacing the
    /// `rendered_lines` snapshot.
    fn recompute_streaming_render(&mut self) {
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&self.raw_source, self.width, &mut rendered);
        self.rendered_lines = rendered;
    }

    /// Advance `enqueued_stable_len` toward the target stable boundary and
    /// enqueue any newly-stable lines into the commit-animation queue. Returns
    /// `true` if new lines were enqueued.
    ///
    /// The target is `rendered_lines.len() - tail_budget`, clamped to at least
    /// `emitted_stable_len` (we never un-emit lines). When a structural rewrite
    /// moves the boundary backward (table appeared retroactively), the queue is
    /// rebuilt from scratch.
    fn sync_stable_queue(&mut self) -> bool {
        let tail_budget = self.active_tail_budget_lines();
        let target_stable_len = self
            .rendered_lines
            .len()
            .saturating_sub(tail_budget)
            .max(self.emitted_stable_len);

        if target_stable_len < self.enqueued_stable_len {
            // A structural rewrite moved the stable boundary backward into queued-but-unemitted
            // lines. Rebuild queue from the latest render snapshot.
            self.state.clear_queue();
            if self.emitted_stable_len < target_stable_len {
                self.state.enqueue(
                    self.rendered_lines[self.emitted_stable_len..target_stable_len].to_vec(),
                );
            }
            self.enqueued_stable_len = target_stable_len;
            return self.state.queued_len() > 0;
        }

        if target_stable_len == self.enqueued_stable_len {
            return false;
        }

        self.state
            .enqueue(self.rendered_lines[self.enqueued_stable_len..target_stable_len].to_vec());
        self.enqueued_stable_len = target_stable_len;
        true
    }

    /// Rebuild the stable queue from the current render snapshot, discarding any
    /// previously queued lines. Used after `set_width()` where the old queue is
    /// stale.
    fn rebuild_stable_queue_from_render(&mut self) {
        let tail_budget = self.active_tail_budget_lines();
        let target_stable_len = self
            .rendered_lines
            .len()
            .saturating_sub(tail_budget)
            .max(self.emitted_stable_len);
        self.state.clear_queue();
        if self.emitted_stable_len < target_stable_len {
            self.state
                .enqueue(self.rendered_lines[self.emitted_stable_len..target_stable_len].to_vec());
        }
        self.enqueued_stable_len = target_stable_len;
    }

    /// How many rendered lines to withhold from the stable queue as mutable
    /// tail. When a table is detected in the source, the entire rendered buffer
    /// is tail; otherwise zero.
    fn active_tail_budget_lines(&self) -> usize {
        match table_holdback_state(&self.raw_source) {
            TableHoldbackState::Confirmed => self.rendered_lines.len(),
            TableHoldbackState::PendingHeader => self.rendered_lines.len(),
            TableHoldbackState::None => 0,
        }
    }

    fn reset_stream_state(&mut self) {
        self.state.clear();
        self.raw_source.clear();
        self.rendered_lines.clear();
        self.enqueued_stable_len = 0;
        self.emitted_stable_len = 0;
    }
}

/// Controller that streams proposed plan markdown into a styled plan block.
pub(crate) struct PlanStreamController {
    state: StreamState,
    width: Option<usize>,
    raw_source: String,
    rendered_lines: Vec<Line<'static>>,
    enqueued_stable_len: usize,
    emitted_stable_len: usize,
    header_emitted: bool,
    top_padding_emitted: bool,
}

impl PlanStreamController {
    pub(crate) fn new(width: Option<usize>) -> Self {
        Self {
            state: StreamState::new(width),
            width,
            raw_source: String::new(),
            rendered_lines: Vec::new(),
            enqueued_stable_len: 0,
            emitted_stable_len: 0,
            header_emitted: false,
            top_padding_emitted: false,
        }
    }

    /// Push a delta; if it contains a newline, commit completed lines and start animation.
    pub(crate) fn push(&mut self, delta: &str) -> bool {
        if !delta.is_empty() {
            self.state.has_seen_delta = true;
        }
        self.state.collector.push_delta(delta);

        let mut enqueued = false;
        if delta.contains('\n')
            && let Some(committed_source) = self.state.collector.commit_complete_source()
        {
            self.raw_source.push_str(&committed_source);
            self.recompute_streaming_render();
            enqueued = self.sync_stable_queue();
        }
        enqueued
    }

    /// Finalize the active stream. Drain and emit now.
    pub(crate) fn finalize(&mut self) -> Option<Box<dyn HistoryCell>> {
        let remainder_source = self.state.collector.finalize_and_drain_source();
        if !remainder_source.is_empty() {
            self.raw_source.push_str(&remainder_source);
        }
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&self.raw_source, self.width, &mut rendered);
        let out_lines = if self.emitted_stable_len >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.emitted_stable_len..].to_vec()
        };

        let out = self.emit(out_lines, true);
        self.reset_plan_stream_state();
        out
    }

    /// Step animation: commit at most one queued line and handle end-of-drain cleanup.
    pub(crate) fn on_commit_tick(&mut self) -> (Option<Box<dyn HistoryCell>>, bool) {
        let step = self.state.step();
        self.emitted_stable_len += step.len();
        (self.emit(step, false), self.state.is_idle())
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
        self.emitted_stable_len += step.len();
        (self.emit(step, false), self.state.is_idle())
    }

    /// Returns the current number of queued plan lines waiting to be displayed.
    pub(crate) fn queued_lines(&self) -> usize {
        self.state.queued_len()
    }

    /// Returns the age of the oldest queued plan line.
    pub(crate) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.state.oldest_queued_age(now)
    }

    /// Update rendering width and rebuild queued stable lines to match the new layout.
    pub(crate) fn set_width(&mut self, width: Option<usize>) {
        if self.width == width {
            return;
        }
        let old_width = self.width;
        self.width = width;
        self.state.collector.set_width(width);
        if self.raw_source.is_empty() {
            return;
        }

        // Recalculate emitted_stable_len for the new width so we don't
        // re-queue lines that were already emitted at the old width.
        if self.emitted_stable_len > 0 {
            let emitted_bytes = source_bytes_for_rendered_count(
                &self.raw_source,
                old_width,
                self.emitted_stable_len,
            );
            let mut emitted_at_new = Vec::new();
            crate::markdown::append_markdown_agent(
                &self.raw_source[..emitted_bytes],
                self.width,
                &mut emitted_at_new,
            );
            self.emitted_stable_len = emitted_at_new.len();
        }

        self.recompute_streaming_render();
        self.rebuild_stable_queue_from_render();
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

    fn recompute_streaming_render(&mut self) {
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&self.raw_source, self.width, &mut rendered);
        self.rendered_lines = rendered;
    }

    fn sync_stable_queue(&mut self) -> bool {
        let tail_budget = self.active_tail_budget_lines();
        let target_stable_len = self
            .rendered_lines
            .len()
            .saturating_sub(tail_budget)
            .max(self.emitted_stable_len);

        if target_stable_len < self.enqueued_stable_len {
            self.state.clear_queue();
            if self.emitted_stable_len < target_stable_len {
                self.state.enqueue(
                    self.rendered_lines[self.emitted_stable_len..target_stable_len].to_vec(),
                );
            }
            self.enqueued_stable_len = target_stable_len;
            return self.state.queued_len() > 0;
        }

        if target_stable_len == self.enqueued_stable_len {
            return false;
        }

        self.state
            .enqueue(self.rendered_lines[self.enqueued_stable_len..target_stable_len].to_vec());
        self.enqueued_stable_len = target_stable_len;
        true
    }

    fn rebuild_stable_queue_from_render(&mut self) {
        let tail_budget = self.active_tail_budget_lines();
        let target_stable_len = self
            .rendered_lines
            .len()
            .saturating_sub(tail_budget)
            .max(self.emitted_stable_len);
        self.state.clear_queue();
        if self.emitted_stable_len < target_stable_len {
            self.state
                .enqueue(self.rendered_lines[self.emitted_stable_len..target_stable_len].to_vec());
        }
        self.enqueued_stable_len = target_stable_len;
    }

    fn active_tail_budget_lines(&self) -> usize {
        match table_holdback_state(&self.raw_source) {
            TableHoldbackState::Confirmed => self.rendered_lines.len(),
            TableHoldbackState::PendingHeader => self.rendered_lines.len(),
            TableHoldbackState::None => 0,
        }
    }

    fn reset_plan_stream_state(&mut self) {
        self.state.clear();
        self.raw_source.clear();
        self.rendered_lines.clear();
        self.enqueued_stable_len = 0;
        self.emitted_stable_len = 0;
    }
}

/// Find the largest newline-terminated prefix of `raw_source` whose
/// rendering at `width` produces at most `target_count` lines.
///
/// When the target falls exactly on a source-line boundary, the returned
/// offset covers that line. When it falls in the middle of a wrapped
/// source line (partial drain), the offset stops at the *previous*
/// newline to avoid overshooting — this may re-queue a few already-emitted
/// wrapped lines as duplicates, but never drops un-emitted content.
///
/// For non-table content (the only case where `emitted_stable_len > 0`),
/// rendering a newline-terminated prefix produces a prefix of the full
/// rendering, so this converges correctly.
fn source_bytes_for_rendered_count(
    raw_source: &str,
    width: Option<usize>,
    target_count: usize,
) -> usize {
    if target_count == 0 {
        return 0;
    }
    let mut best_offset = 0;
    for (i, _) in raw_source.match_indices('\n') {
        let prefix = &raw_source[..=i];
        let mut lines = Vec::new();
        crate::markdown::append_markdown_agent(prefix, width, &mut lines);
        if lines.len() <= target_count {
            best_offset = i + 1;
        }
        if lines.len() >= target_count {
            break;
        }
    }
    best_offset
}

fn parse_fence_marker(line: &str) -> Option<(char, usize)> {
    let mut chars = line.chars();
    let first = chars.next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let mut len = 1usize;
    for ch in chars {
        if ch == first {
            len += 1;
        } else {
            break;
        }
    }
    if len < 3 {
        return None;
    }
    Some((first, len))
}

/// A source line annotated with whether it falls inside a fenced code block.
struct ParsedLine<'a> {
    text: &'a str,
    fence_context: FenceContext,
}

/// Where a source line sits relative to fenced code blocks.
///
/// Table holdback only applies to lines that are `Outside` or inside a
/// `Markdown` fence. Lines inside `Other` fences (e.g. `sh`, `rust`) are
/// ignored by the table scanner because their pipe characters are code, not
/// table syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FenceContext {
    /// Not inside any fenced code block.
    Outside,
    /// Inside a `` ```md `` or `` ```markdown `` fence.
    Markdown,
    /// Inside a fence with a non-markdown info string.
    Other,
}

fn is_markdown_fence(trimmed_line: &str, marker_len: usize) -> bool {
    let info = trimmed_line[marker_len..]
        .split_whitespace()
        .next()
        .unwrap_or_default();
    info.eq_ignore_ascii_case("md") || info.eq_ignore_ascii_case("markdown")
}

fn parse_lines_with_fence_state(source: &str) -> Vec<ParsedLine<'_>> {
    let mut in_fence = false;
    let mut fence_char = '\0';
    let mut fence_context = FenceContext::Other;
    let mut lines = Vec::new();

    for raw_line in source.split('\n') {
        lines.push(ParsedLine {
            text: raw_line,
            fence_context: if in_fence {
                fence_context
            } else {
                FenceContext::Outside
            },
        });

        let trimmed = raw_line.trim_start();
        if let Some((marker, len)) = parse_fence_marker(trimmed) {
            if !in_fence {
                in_fence = true;
                fence_char = marker;
                fence_context = if is_markdown_fence(trimmed, len) {
                    FenceContext::Markdown
                } else {
                    FenceContext::Other
                };
            } else if marker == fence_char && len >= 3 {
                in_fence = false;
                fence_context = FenceContext::Other;
            }
        }
    }

    lines
}

fn strip_blockquote_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let Some(stripped) = rest.strip_prefix('>') else {
            return rest;
        };
        rest = stripped.strip_prefix(' ').unwrap_or(stripped).trim_start();
    }
}

fn table_candidate_text(line: &str) -> Option<&str> {
    let stripped = strip_blockquote_prefix(line).trim();
    parse_table_segments(stripped).map(|_| stripped)
}

use crate::table_detect::is_table_header_line;
use crate::table_detect::is_table_delimiter_line;
use crate::table_detect::parse_table_segments;

/// Whether the accumulated raw source contains a markdown table that requires
/// holdback of the mutable tail to prevent partial-table commits.
enum TableHoldbackState {
    /// No table detected -- all rendered lines can flow into the stable queue.
    None,
    /// The last non-blank line looks like a table header row but no delimiter
    /// row has followed yet. Hold back in case the next delta is a delimiter.
    PendingHeader,
    /// A header + delimiter pair was found -- the source contains a confirmed
    /// table. The entire rendered buffer is held as mutable tail.
    Confirmed,
}

/// Scan `source` for pipe-table patterns (header row followed by delimiter row)
/// outside of non-markdown fenced code blocks. Used by the stream controllers
/// to decide the tail budget.
fn table_holdback_state(source: &str) -> TableHoldbackState {
    let lines = parse_lines_with_fence_state(source);
    for pair in lines.windows(2) {
        let [header_line, delimiter_line] = pair else {
            continue;
        };
        if header_line.fence_context == FenceContext::Other
            || delimiter_line.fence_context == FenceContext::Other
        {
            continue;
        }

        let Some(header_text) = table_candidate_text(header_line.text) else {
            continue;
        };
        let Some(delimiter_text) = table_candidate_text(delimiter_line.text) else {
            continue;
        };

        if is_table_header_line(header_text) && is_table_delimiter_line(delimiter_text) {
            return TableHoldbackState::Confirmed;
        }
    }

    let pending_header = lines.iter().rev().find(|line| !line.text.trim().is_empty());
    let pending_header = pending_header.is_some_and(|line| {
        line.fence_context != FenceContext::Other
            && table_candidate_text(line.text).is_some_and(is_table_header_line)
    });
    if pending_header {
        TableHoldbackState::PendingHeader
    } else {
        TableHoldbackState::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn collect_streamed_lines(deltas: &[&str], width: Option<usize>) -> Vec<String> {
        let mut ctrl = StreamController::new(width);
        let mut lines = Vec::new();
        for d in deltas {
            ctrl.push(d);
            while let (Some(cell), idle) = ctrl.on_commit_tick() {
                lines.extend(cell.transcript_lines(u16::MAX));
                if idle {
                    break;
                }
            }
        }
        if let (Some(cell), _source) = ctrl.finalize() {
            lines.extend(cell.transcript_lines(u16::MAX));
        }
        lines_to_plain_strings(&lines)
            .into_iter()
            .map(|s| s.chars().skip(2).collect::<String>())
            .collect()
    }

    fn collect_plan_streamed_lines(deltas: &[&str], width: Option<usize>) -> Vec<String> {
        let mut ctrl = PlanStreamController::new(width);
        let mut lines = Vec::new();
        for d in deltas {
            ctrl.push(d);
            while let (Some(cell), idle) = ctrl.on_commit_tick() {
                lines.extend(cell.transcript_lines(u16::MAX));
                if idle {
                    break;
                }
            }
        }
        if let Some(cell) = ctrl.finalize() {
            lines.extend(cell.transcript_lines(u16::MAX));
        }
        lines_to_plain_strings(&lines)
    }

    #[test]
    fn controller_set_width_rebuilds_queued_lines() {
        let mut ctrl = StreamController::new(Some(120));
        let delta = "This is a long line that should wrap into multiple rows when resized.\n";
        assert!(ctrl.push(delta));
        assert_eq!(ctrl.queued_lines(), 1);

        ctrl.set_width(Some(24));
        let (cell, idle) = ctrl.on_commit_tick_batch(usize::MAX);
        let rendered = lines_to_plain_strings(
            &cell
                .expect("expected resized queued lines")
                .transcript_lines(u16::MAX),
        );

        assert!(idle);
        assert!(
            rendered.len() > 1,
            "expected resized content to occupy multiple lines, got {rendered:?}",
        );
    }

    #[test]
    fn controller_set_width_no_duplicate_after_emit() {
        // Regression: if lines were already emitted at the old width and the
        // terminal shrinks (causing those lines to expand), the queue must not
        // re-enqueue the expanded suffix of already-emitted content.
        let mut ctrl = StreamController::new(Some(120));
        let line = "This is a long line that definitely wraps when the terminal shrinks to 24 columns.\n";
        ctrl.push(line);
        // Drain the queue — this content is now "emitted" (in transcript_cells).
        let (cell, _) = ctrl.on_commit_tick_batch(usize::MAX);
        assert!(cell.is_some(), "expected emitted cell");
        assert_eq!(ctrl.queued_lines(), 0);

        // Shrink the terminal — the emitted line now wraps to multiple lines.
        ctrl.set_width(Some(24));

        // Nothing should be queued because the content was already emitted.
        assert_eq!(
            ctrl.queued_lines(),
            0,
            "already-emitted content must not be re-queued after resize",
        );
    }

    #[test]
    fn controller_set_width_partial_drain_no_lost_lines() {
        // Regression: if a wrapped source line is partially drained (some
        // wrapped lines emitted, others still queued) and then the terminal
        // resizes, the un-emitted wrapped lines must not be lost.
        let mut ctrl = StreamController::new(Some(40));
        // Push two source lines. The first is long enough to wrap at width=40.
        ctrl.push("AAAA BBBB CCCC DDDD EEEE FFFF GGGG HHHH IIII JJJJ\n");
        ctrl.push("second line\n");

        // Drain only 1 rendered line (the first wrapped row of line 1).
        let (cell, idle) = ctrl.on_commit_tick();
        assert!(cell.is_some(), "expected 1 emitted line");
        assert!(!idle, "queue should still have lines");
        let remaining_before = ctrl.queued_lines();
        assert!(remaining_before > 0, "should have queued lines left");

        // Resize to a narrower width.
        ctrl.set_width(Some(20));

        // Finalize and collect all remaining output.
        let (cell, source) = ctrl.finalize();
        let final_lines = cell
            .map(|c| lines_to_plain_strings(&c.transcript_lines(u16::MAX)))
            .unwrap_or_default();

        // The finalized output must contain "second line" — it was queued
        // and must not be dropped by the resize remapping.
        assert!(
            final_lines.iter().any(|l| l.contains("second line")),
            "un-emitted 'second line' was lost after resize; got: {final_lines:?}",
        );
        assert!(source.is_some(), "expected source from finalize");
    }

    #[test]
    fn controller_set_width_preserves_in_flight_tail() {
        let mut ctrl = StreamController::new(Some(80));
        ctrl.push("tail without newline");
        ctrl.set_width(Some(24));

        let (cell, _source) = ctrl.finalize();
        let rendered = lines_to_plain_strings(
            &cell
                .expect("expected finalized tail")
                .transcript_lines(u16::MAX),
        );

        assert_eq!(rendered, vec!["• tail without newline".to_string()]);
    }

    #[test]
    fn plan_controller_set_width_preserves_in_flight_tail() {
        let mut ctrl = PlanStreamController::new(Some(80));
        ctrl.push("1. Item without newline");
        ctrl.set_width(Some(24));

        let rendered = lines_to_plain_strings(
            &ctrl
                .finalize()
                .expect("expected finalized tail")
                .transcript_lines(u16::MAX),
        );

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("Item without newline")),
            "expected finalized plan content after resize, got {rendered:?}",
        );
    }

    #[tokio::test]
    async fn controller_loose_vs_tight_with_commit_ticks_matches_full() {
        let mut ctrl = StreamController::new(None);
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
        if let (Some(cell), _source) = ctrl.finalize() {
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
        crate::markdown::append_markdown_agent(&source, None, &mut rendered);
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
    async fn controller_streamed_table_matches_full_render_widths() {
        let deltas = vec![
            "| Key | Description |\n",
            "| --- | --- |\n",
            "| -v | Enable very verbose logging output for debugging |\n",
            "\n",
        ];

        let streamed = collect_streamed_lines(&deltas, Some(80));

        let source: String = deltas.iter().copied().collect();
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&source, Some(80), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
    }

    #[tokio::test]
    async fn controller_holds_blockquoted_table_tail_until_stable() {
        let deltas = vec![
            "> | A | B |\n",
            "> | --- | --- |\n",
            "> | longvalue | ok |\n",
            "\n",
        ];

        let streamed = collect_streamed_lines(&deltas, Some(80));

        let source: String = deltas.iter().copied().collect();
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&source, Some(80), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
    }

    #[tokio::test]
    async fn controller_does_not_hold_back_pipe_prose_without_table_delimiter() {
        let mut ctrl = StreamController::new(Some(80));

        ctrl.push("status | owner | note\n");
        let (_first_commit, first_idle) = ctrl.on_commit_tick();
        assert!(first_idle);

        ctrl.push("next line\n");
        let (second_commit, _second_idle) = ctrl.on_commit_tick();
        assert!(
            second_commit.is_some(),
            "expected prose lines to be released once no table delimiter follows"
        );
    }

    #[tokio::test]
    async fn controller_handles_table_immediately_after_heading() {
        let deltas = vec![
            "### 1) Basic table\n",
            "| Name | Role | Status |\n",
            "|---|---|---|\n",
            "| Alice | Admin | Active |\n",
            "| Bob | Editor | Pending |\n",
            "\n",
        ];

        let streamed = collect_streamed_lines(&deltas, Some(100));

        let source: String = deltas.iter().copied().collect();
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&source, Some(100), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
    }

    #[tokio::test]
    async fn controller_renders_unicode_for_multi_table_response_shape() {
        let source = "Absolutely. Here are several different Markdown table patterns you can use for rendering tests.\n\n| Name  | Role      | Location |\n|-------|-----------|----------|\n| Ava   | Engineer  | NYC      |\n| Malik | Designer  | Berlin   |\n| Priya | PM        | Remote   |\n\n| Item        | Qty | Price | In Stock |\n|:------------|----:|------:|:--------:|\n| Keyboard    |   2 | 49.99 |    Yes   |\n| Mouse       |  10 | 19.50 |    Yes   |\n| Monitor     |   1 | 219.0 |    No    |\n\n| Field         | Example                         | Notes                    |\n|---------------|----------------------------------|--------------------------|\n| Escaped pipe  | `foo \\| bar`                    | Should stay in one cell  |\n| Inline code   | `let x = value;`                | Monospace inline content |\n| Link          | [OpenAI](https://openai.com)    | Standard markdown link   |\n";

        let chunked = source
            .split_inclusive('\n')
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let deltas = chunked.iter().map(String::as_str).collect::<Vec<_>>();
        let streamed = collect_streamed_lines(&deltas, Some(120));
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border in streamed output: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_renders_unicode_for_no_outer_pipes_table_shape() {
        let source = "### 1) Basic\n\n| Name | Role | Active |\n|---|---|---|\n| Alice | Engineer | Yes |\n| Bob | Designer | No |\n\n### 2) No outer pipes\n\nCol A | Col B | Col C\n--- | --- | ---\nx | y | z\n10 | 20 | 30\n\n### 3) Another table\n\n| Key | Value |\n|---|---|\n| a | b |\n";

        let chunked = source
            .split_inclusive('\n')
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let deltas = chunked.iter().map(String::as_str).collect::<Vec<_>>();
        let streamed = collect_streamed_lines(&deltas, Some(100));

        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(source, Some(100), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
        let has_raw_no_outer_header = streamed
            .iter()
            .any(|line| line.trim() == "Col A | Col B | Col C");
        assert!(
            !has_raw_no_outer_header,
            "no-outer-pipes header should not remain raw in final streamed output: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_stabilizes_first_no_outer_pipes_table_in_response() {
        let deltas = vec![
            "### No outer pipes first\n\n",
            "Col A | Col B | Col C\n",
            "--- | --- | ---\n",
            "x | y | z\n",
            "10 | 20 | 30\n",
            "\n",
            "After table paragraph.\n",
        ];
        let streamed = collect_streamed_lines(&deltas, Some(100));

        let source: String = deltas.iter().copied().collect();
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&source, Some(100), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border for no-outer-pipes streaming: {streamed:?}"
        );
        assert!(
            !streamed
                .iter()
                .any(|line| line.trim() == "Col A | Col B | Col C"),
            "did not expect raw no-outer-pipes header in final streamed output: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_stabilizes_two_column_no_outer_table_in_response() {
        let deltas = vec![
            "A | B\n",
            "--- | ---\n",
            "left | right\n",
            "\n",
            "After table paragraph.\n",
        ];
        let streamed = collect_streamed_lines(&deltas, Some(80));

        let source: String = deltas.iter().copied().collect();
        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(&source, Some(80), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border for two-column no-outer table: {streamed:?}"
        );
        assert!(
            !streamed.iter().any(|line| line.trim() == "A | B"),
            "did not expect raw two-column no-outer header in final streamed output: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_converts_no_outer_table_between_preboxed_sections() {
        let source = "  ┌───────┬──────────┬────────┐\n  │ Name  │ Role     │ Active │\n  ├───────┼──────────┼────────┤\n  │ Alice │ Engineer │ Yes    │\n  │ Bob   │ Designer │ No     │\n  │ Cara  │ PM       │ Yes    │\n  └───────┴──────────┴────────┘\n\n  ### 3) No outer pipes\n\n  Col A | Col B | Col C\n  --- | --- | ---\n  x | y | z\n  10 | 20 | 30\n\n  ┌─────────────────┬────────┬────────────────────────┐\n  │ Example         │ Output │ Notes                  │\n  ├─────────────────┼────────┼────────────────────────┤\n  │ a | b           │ `a     │ b`                     │\n  │ npm run test    │ ok     │ Inline code formatting │\n  │ SELECT * FROM t │ 3 rows │ SQL snippet            │\n  └─────────────────┴────────┴────────────────────────┘\n";

        let deltas = source
            .split_inclusive('\n')
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let streamed = collect_streamed_lines(
            &deltas.iter().map(String::as_str).collect::<Vec<_>>(),
            Some(100),
        );

        let has_raw_no_outer_header = streamed
            .iter()
            .any(|line| line.trim() == "Col A | Col B | Col C");
        assert!(
            !has_raw_no_outer_header,
            "no-outer table header remained raw in streamed output: {streamed:?}"
        );
        assert!(
            streamed
                .iter()
                .any(|line| line.contains("┌───────┬───────┬───────┐")),
            "expected converted no-outer table border in streamed output: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_keeps_markdown_fenced_tables_mutable_until_finalize() {
        let source = "```md\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";
        let deltas = vec![
            "```md\n",
            "| A | B |\n",
            "|---|---|\n",
            "| 1 | 2 |\n",
            "```\n",
        ];
        let streamed = collect_streamed_lines(&deltas, Some(80));

        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(source, Some(80), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border in streamed output: {streamed:?}"
        );
        assert!(
            !streamed.iter().any(|line| line.trim() == "| A | B |"),
            "did not expect raw table header line after finalize: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_keeps_markdown_fenced_no_outer_tables_mutable_until_finalize() {
        let source =
            "```md\nCol A | Col B | Col C\n--- | --- | ---\nx | y | z\n10 | 20 | 30\n```\n";
        let deltas = vec![
            "```md\n",
            "Col A | Col B | Col C\n",
            "--- | --- | ---\n",
            "x | y | z\n",
            "10 | 20 | 30\n",
            "```\n",
        ];
        let streamed = collect_streamed_lines(&deltas, Some(100));

        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(source, Some(100), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border in streamed output: {streamed:?}"
        );
        assert!(
            !streamed
                .iter()
                .any(|line| line.trim() == "Col A | Col B | Col C"),
            "did not expect raw no-outer-pipes header line after finalize: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn controller_keeps_non_markdown_fenced_tables_as_code() {
        let source = "```sh\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";
        let deltas = vec![
            "```sh\n",
            "| A | B |\n",
            "|---|---|\n",
            "| 1 | 2 |\n",
            "```\n",
        ];
        let streamed = collect_streamed_lines(&deltas, Some(80));

        let mut rendered = Vec::new();
        crate::markdown::append_markdown_agent(source, Some(80), &mut rendered);
        let expected = lines_to_plain_strings(&rendered);

        assert_eq!(streamed, expected);
        assert!(
            streamed.iter().any(|line| line.trim() == "| A | B |"),
            "expected code-fenced pipe line to remain raw: {streamed:?}"
        );
        assert!(
            !streamed.iter().any(|line| line.contains('┌')),
            "did not expect unicode table border for non-markdown fence: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn plan_controller_streamed_table_matches_finalize_render() {
        let deltas = vec![
            "## Build plan\n\n",
            "| Step | Owner |\n",
            "|---|---|\n",
            "| Write tests | Agent |\n",
            "| Verify output | User |\n",
            "\n",
        ];
        let streamed = collect_plan_streamed_lines(&deltas, Some(80));

        let source: String = deltas.iter().copied().collect();
        let baseline = collect_plan_streamed_lines(&[source.as_str()], Some(80));

        assert_eq!(streamed, baseline);
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border in plan streamed output: {streamed:?}"
        );
        assert!(
            !streamed
                .iter()
                .any(|line| line.trim() == "| Step | Owner |"),
            "did not expect raw table header line in plan output: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn plan_controller_streamed_markdown_fenced_table_matches_finalize_render() {
        let deltas = vec![
            "## Build plan\n\n",
            "```md\n",
            "| Step | Owner |\n",
            "|---|---|\n",
            "| Write tests | Agent |\n",
            "| Verify output | User |\n",
            "```\n",
            "\n",
        ];
        let streamed = collect_plan_streamed_lines(&deltas, Some(80));

        let source: String = deltas.iter().copied().collect();
        let baseline = collect_plan_streamed_lines(&[source.as_str()], Some(80));

        assert_eq!(streamed, baseline);
        assert!(
            streamed.iter().any(|line| line.contains('┌')),
            "expected unicode table border in fenced plan output: {streamed:?}"
        );
    }

    #[test]
    fn table_holdback_state_detects_header_plus_delimiter() {
        let source = "| Key | Description |\n| --- | --- |\n";
        assert!(matches!(
            table_holdback_state(source),
            TableHoldbackState::Confirmed
        ));
    }
}
