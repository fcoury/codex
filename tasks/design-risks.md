# Design Risks and Implicit Contracts

## Implicit Contracts

### Enforced by types

1. **`StreamCore` fields are private.** All access goes through methods, so
   the `emitted_stable_len <= enqueued_stable_len <= rendered_lines.len()`
   invariant is maintained by the implementation, not by callers.

2. **`TableHoldbackState` is a plain enum.** The holdback decision is pure
   function of `raw_source` — no mutable state tracks transitions, so there is
   no risk of stale holdback state across deltas.

3. **`AgentMarkdownCell` stores only `String`.** Re-rendering is forced on
   every `display_lines(width)` call, so there is no cached render that could
   become stale after a width change.

### Enforced by tests

4. **Table detection correctness.** `table_detect.rs` tests cover segment
   parsing, delimiter syntax (including alignment colons), and edge cases
   (single segment, empty lines).

5. **Fence unwrapping selectivity.** Tests verify that `md`/`markdown` fences
   with tables are unwrapped, non-markdown fences are preserved, and
   non-table markdown fences are preserved.

6. **Column width allocation.** `markdown_render_tests.rs` exercises
   natural-fit, narrow-terminal, and minimum-width scenarios.

### Social/tribal knowledge only

7. **`source_bytes_for_rendered_count` convergence assumption.** The function
   assumes that for non-table content, rendering a newline-terminated source
   prefix produces a *prefix* of the full render.  This is true for the
   current `pulldown-cmark` → `Writer` pipeline but is not tested in
   isolation.  If a future markdown feature (e.g. footnotes, multi-paragraph
   list items) violates this assumption, resize remapping will produce
   duplicated or dropped lines.

   *Recommendation:* Add a targeted unit test for
   `source_bytes_for_rendered_count` with plain text and list content to lock
   in the prefix-stability invariant.

8. **`reflow_ran_during_stream` lifecycle.** The flag is set during resize
   inside an agent stream and cleared in `ConsolidateAgentMessage`.  If the
   stream is interrupted (e.g. abort, thread switch) without producing a
   consolidation event, the flag remains set.  The Phase 4 fix added clearing
   in the `else` branch, but there may be other abort paths that skip
   `ConsolidateAgentMessage` entirely.

   *Recommendation:* Document in `app.rs` which events clear the flag, or
   consolidate the flag-clearing into `reset_for_new_thread()`.

9. **Table holdback applies to the entire buffer, not per-table regions.** If
   raw source contains mixed prose + table, the prose lines are also withheld
   during streaming.  This is conservative and correct but can delay output.

   *Recommendation:* Document this behavior in the `active_tail_budget_lines`
   doc comment. No code change needed unless users report perceived latency.

10. **`unwrap_markdown_fences` and `table_holdback_state` both scan for table
    patterns** but at different levels: the former operates on fence content
    (pre-render), the latter on the full accumulated source (mid-stream).  A
    bug fix in one may need replication in the other.

    *Recommendation:* Both already delegate to `table_detect::*` for the
    primitive checks, which mitigates this.  The higher-level scanning logic
    (fence-awareness in `parse_lines_with_fence_state` vs. simple
    `content.lines()` in `markdown_fence_contains_table`) still differs.
    Consider extracting a shared "does this content contain a table" predicate
    if the scanning logic diverges further.

---

## Debug Path

### Where state lives

| Component | State | Location |
|-----------|-------|----------|
| Raw markdown source | `StreamCore.raw_source` | `streaming/controller.rs` |
| Full render snapshot | `StreamCore.rendered_lines` | `streaming/controller.rs` |
| Stable/tail partition | `enqueued_stable_len`, `emitted_stable_len` | `streaming/controller.rs` |
| Animation queue | `StreamState.queued_lines` | `streaming/mod.rs` |
| Incomplete line buffer | `MarkdownStreamCollector.buffer` | `markdown_stream.rs` |
| Consolidated source | `AgentMarkdownCell.markdown_source` | `history_cell.rs` |
| Reflow-during-stream flag | `App.reflow_ran_during_stream` | `app.rs` |
| Resize debounce timer | `App.resize_reflow_pending_until` | `app.rs` |

### Tracing entry points

- `ConsolidateAgentMessage` handler logs cell range and whether reflow is
  scheduled (`tracing::debug!`).
- No tracing in `table_holdback_state` or `active_tail_budget_lines`.  Add
  `tracing::trace!` at the `Confirmed` / `PendingHeader` return sites for
  streaming-holdback debugging.
- No tracing in `source_bytes_for_rendered_count`.  The function is called
  only on resize, so a `tracing::debug!` with `(emitted_stable_len,
  emitted_bytes, new_emitted_stable_len)` would be low-noise and useful.

### How to trace a "table looks wrong after resize"

1. Enable `RUST_LOG=codex_tui=debug`.
2. Look for `ConsolidateAgentMessage` log to confirm consolidation happened.
3. If consolidation did not fire, the stream may have been interrupted.  Check
   for `reflow_ran_during_stream` flag state.
4. If consolidation fired but table is still wrong, the issue is in
   `AgentMarkdownCell.display_lines(width)` — run the markdown source through
   `append_markdown_agent` at the new width in a test to reproduce.

---

## Commit-Story Improvements

The branch has 14 commits.  The first ~8 add feature code and the last ~6
are fix-ups.  Suggested rewrite for a squash-merge PR description:

| Current message | Suggested rewrite |
|----------------|-------------------|
| "Fix resize mid-drain dropping un-emitted wrapped lines" | "Preserve un-emitted wrapped lines when terminal resize occurs during commit-animation drain" |
| "Restrict reflow flag to agent message streams only" | "Scope reflow-during-stream flag to agent streams to prevent spurious reflows during plan streaming" |
| "Fix resize mid-stream duplicating already-emitted lines" | "Remap emitted line count through source bytes on resize to prevent duplicate scrollback lines" |
| "Guard reflow flag to active streams and reset on thread switch" | "Clear reflow flag on consolidation (including no-cells-to-consolidate path) to prevent stale flag leaking across streams" |

For a squash merge, the single commit message should be:

> Add native pipe-table rendering with streaming holdback and resize reflow
>
> Tables in agent responses are now rendered with Unicode box-drawing borders
> and adaptive column widths.  During streaming, table output is held back
> until the table is complete to avoid partial-table artifacts.  After
> finalization, the raw markdown source is preserved in AgentMarkdownCell so
> tables reflow correctly on terminal resize.
