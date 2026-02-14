# PR: Native pipe-table rendering with streaming holdback and resize reflow

## Problem

The Codex TUI renders all markdown through `pulldown-cmark`, but before this
change, tables were displayed as raw pipe-delimited text or, worse, as fenced
code blocks when the LLM wraps them in `` ```md `` fences.  Tables are one of
the most frequent structured outputs from coding assistants (dependency
comparisons, API summaries, permission matrices), so rendering them as
monospace pipe soup wastes horizontal space and makes them hard to scan.

## Mental model

There are three cooperating subsystems; understanding each in isolation is
enough to review the change:

1. **Render pipeline** (`markdown_render.rs`).  `pulldown-cmark` now parses
   with `ENABLE_TABLES`.  A `Writer` accumulates `TableState` during
   `Tag::Table..TagEnd::Table`, then hands it to `render_table_lines()` which
   allocates column widths, wraps cell content, and draws Unicode box-drawing
   borders (┌─┬─┐ / │ │ / └─┴─┘).  Column widths are classified as Narrative
   (long prose, shrunk first) or Structured (short tokens, preserved).  When
   the table cannot fit even at minimum widths, it falls back to plain pipe
   format.

2. **Streaming holdback** (`streaming/controller.rs`).  During streaming, a
   partial table must not be committed to scrollback line-by-line because each
   new row can change column widths and thus reshape every prior line.  The
   `table_holdback_state()` scanner detects pipe-table patterns in the
   accumulated raw source and withholds the entire rendered buffer as mutable
   "tail" until the table is complete.  Non-table content flows to scrollback
   immediately.

3. **Resize reflow** (`history_cell.rs` `AgentMarkdownCell`, `app.rs`).  After
   a stream finalizes, the run of `AgentMessageCell` chunks is consolidated
   into a single `AgentMarkdownCell` that stores raw markdown source and
   re-renders from it at any width on `display_lines(width)`.  Terminal resize
   re-renders all transcript cells at the new width, so tables reflow their
   column widths and word-wrapping automatically.

## Non-goals

- **Syntax highlighting inside table cells.** Cells use the same inline style
  stack as regular markdown (bold, code, links) but do not syntax-highlight
  code blocks within cells.
- **Horizontal scrolling.** If a table cannot fit at minimum column widths, it
  falls back to pipe format rather than introducing a scrollable viewport.
- **Streaming table "preview".** During holdback the entire table is tail
  (visible in the active-cell slot), but it is not committed to scrollback
  until complete.  There is no incremental row-by-row stable display.

## Tradeoffs

| Decision | Upside | Downside |
|----------|--------|----------|
| Holdback entire buffer when any table detected | Simple implementation; avoids partial-table scrollback artifacts | Large tables delay all scrollback output until the table is complete |
| Narrative/Structured column classification | Prose columns shrink gracefully; structured columns stay readable | Heuristic (>=4 avg words or >=28 avg chars) can misclassify |
| Store raw markdown source in `AgentMarkdownCell` | Perfect reflow on resize; single source of truth | Memory cost of storing source + rendered lines (acceptable for typical message sizes) |
| `unwrap_markdown_fences` pre-pass | Handles LLMs that wrap tables in `` ```md `` fences | Additional string pass before rendering; fence detection duplicates some table-detection logic |

## Architecture

```
LLM delta tokens
    │
    ▼
MarkdownStreamCollector          (newline-gated buffering)
    │
    ▼
StreamCore.push_delta()          (commit completed source, re-render)
    │
    ├── table_holdback_state()   (scan for pipe-table patterns)
    │       │
    │       ▼
    ├── sync_stable_queue()      (enqueue stable lines, withhold tail)
    │
    ▼
StreamController.emit()          (wrap as AgentMessageCell → scrollback)
    │
    ▼  (on finalize)
ConsolidateAgentMessage          (replace cell run with AgentMarkdownCell)
    │
    ▼  (on terminal resize)
AgentMarkdownCell.display_lines(new_width)  → re-render from source
```

Key modules and their responsibilities:

| Module | Responsibility |
|--------|---------------|
| `table_detect.rs` | Canonical pipe-table pattern matching (shared by fence unwrapper and holdback scanner) |
| `markdown.rs` | Entry points `append_markdown` / `append_markdown_agent`; fence unwrapping |
| `markdown_render.rs` | `pulldown-cmark` → `Vec<Line>` with table layout, column width allocation, box-drawing |
| `markdown_stream.rs` | `MarkdownStreamCollector`: newline-gated source buffering for streaming |
| `streaming/controller.rs` | `StreamCore` + `StreamController` / `PlanStreamController`: two-region model, holdback, resize remapping |
| `streaming/mod.rs` | `StreamState`: FIFO queue of committed lines with timestamps |
| `history_cell.rs` | `AgentMarkdownCell` (source-backed reflow), `StreamingAgentTailCell` (live tail preview) |
| `app.rs` | `ConsolidateAgentMessage` handler, `reflow_ran_during_stream` flag, resize debouncing |

## Observability

- `tracing::debug!` in `ConsolidateAgentMessage` logs cell range being replaced
  and whether reflow is scheduled.
- Table holdback decisions are implicit in `active_tail_budget_lines()` return
  value but not traced.  Adding a trace at `Confirmed`/`PendingHeader`
  transitions would aid debugging without log spam.
- `MarkdownStreamCollector` width changes are observable through
  `committed_source_len` tracking but not traced.

## Tests

892 tests pass in `codex-tui`.  Key coverage areas:

- **Table detection** (`table_detect.rs`): segment parsing, header/delimiter
  validation, alignment-colon syntax.
- **Fence unwrapping** (`markdown.rs`): `md`/`markdown` fences with tables are
  unwrapped; non-markdown fences preserved; non-table markdown fences preserved.
- **Column width allocation** (`markdown_render_tests.rs`): natural-fit tables,
  narrow-terminal shrinking, narrative vs structured classification, pipe
  fallback, spillover row detection.
- **Streaming controller** (`controller.rs`): holdback state machine, resize
  remapping via `source_bytes_for_rendered_count`, queue sync after rewrite,
  table-aware tail budget.
- **No direct test for `source_bytes_for_rendered_count`** as a standalone
  function.  It is exercised indirectly through `set_width` tests.
- **No integration test for the full resize reflow path** (resize → re-render
  transcript cells → `AgentMarkdownCell.display_lines(new_width)`).
