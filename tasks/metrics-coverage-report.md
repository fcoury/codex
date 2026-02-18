# Metrics Coverage Analysis Report

## Executive Summary

The codex-rs codebase has **47 named metrics** across the `codex-otel`, `codex-core`, and `codex-state` crates. Coverage is strong for API/SSE/WebSocket transport, Responses API timing, and the otel_manager event-recording hub, but significant gaps exist in tool execution, error classification, sandbox operations (non-Windows), and the TUI/CLI layers.

## Current Metrics Inventory (47 metrics)

### Transport & API (16 metrics) — WELL COVERED
| Metric | Type | Source |
|---|---|---|
| `codex.api_request` | counter | `otel_manager.rs` |
| `codex.api_request.duration_ms` | histogram | `otel_manager.rs` |
| `codex.sse_event` | counter | `otel_manager.rs` |
| `codex.sse_event.duration_ms` | histogram | `otel_manager.rs` |
| `codex.websocket.request` | counter | `otel_manager.rs` |
| `codex.websocket.request.duration_ms` | histogram | `otel_manager.rs` |
| `codex.websocket.event` | counter | `otel_manager.rs` |
| `codex.websocket.event.duration_ms` | histogram | `otel_manager.rs` |
| `codex.responses_api_overhead.duration_ms` | histogram | `otel_manager.rs` |
| `codex.responses_api_inference_time.duration_ms` | histogram | `otel_manager.rs` |
| `codex.responses_api_engine_iapi_ttft.duration_ms` | histogram | `otel_manager.rs` |
| `codex.responses_api_engine_service_ttft.duration_ms` | histogram | `otel_manager.rs` |
| `codex.responses_api_engine_iapi_tbt.duration_ms` | histogram | `otel_manager.rs` |
| `codex.responses_api_engine_service_tbt.duration_ms` | histogram | `otel_manager.rs` |
| `codex.transport.fallback_to_http` | counter | `client.rs` |
| `codex.tool.call` / `codex.tool.call.duration_ms` | counter+histogram | `otel_manager.rs` |

### Session & Tasks (8 metrics) — MODERATE COVERAGE
| Metric | Type | Source |
|---|---|---|
| `codex.thread.started` | counter | `codex.rs` |
| `codex.model_warning` | counter | `codex.rs` |
| `codex.conversation.turn.count` | counter | `codex.rs` |
| `codex.turn.e2e_duration_ms` | timer | `tasks/mod.rs` |
| `codex.task.compact` | counter | `tasks/compact.rs` |
| `codex.task.undo` | counter | `tasks/undo.rs` |
| `codex.task.review` | counter | `tasks/review.rs` |
| `codex.task.user_shell` | counter | `tasks/user_shell.rs` |

### Memory System (5 metrics) — WELL COVERED
| Metric | Type | Source |
|---|---|---|
| `codex.memory.phase1` | counter | `memories/phase1.rs` |
| `codex.memory.phase1.output` | counter | `memories/phase1.rs` |
| `codex.memory.phase1.token_usage` | histogram | `memories/phase1.rs` |
| `codex.memory.phase2` | counter | `memories/phase2.rs` |
| `codex.memory.phase2.input` | counter | `memories/phase2.rs` |

### MCP (4 metrics) — MODERATE COVERAGE
| Metric | Type | Source |
|---|---|---|
| `codex.mcp.call` | counter | `mcp_tool_call.rs` |
| `codex.mcp.tools.list.duration_ms` | histogram | `mcp_connection_manager.rs` |
| `codex.mcp.tools.fetch_uncached.duration_ms` | histogram | `mcp_connection_manager.rs` |
| `codex.mcp.tools.cache_write.duration_ms` | histogram | `mcp_connection_manager.rs` |

### Database (5 metrics) — WELL COVERED
| Metric | Type | Source |
|---|---|---|
| `codex.db.init` | counter | `state/lib.rs` |
| `codex.db.error` | counter | `state/lib.rs` |
| `codex.db.backfill` | counter | `state/lib.rs` |
| `codex.db.backfill.duration_ms` | histogram | `state/lib.rs` |
| `codex.db.compare_error` | counter | `state/lib.rs` |

### Sandbox, Features, Misc (9 metrics)
| Metric | Type | Source |
|---|---|---|
| `codex.shell_snapshot` | counter | `shell_snapshot.rs` |
| `codex.shell_snapshot.duration_ms` | timer | `shell_snapshot.rs` |
| `codex.approval.requested` | counter | `tools/sandboxing.rs` |
| `codex.feature.state` | counter | `features.rs` |
| `codex.skill.injected` | counter | `skills/injection.rs` |
| `codex.remote_models.fetch_update.duration_ms` | timer | `models_manager/manager.rs` |
| `codex.remote_models.load_cache.duration_ms` | timer | `models_manager/manager.rs` |
| `codex.windows_sandbox.elevated_setup_*` | counters | `windows_sandbox.rs` |
| `codex.windows_sandbox.createprocessasuserw_failed` | counter | `exec.rs` |

---

## Coverage Gaps — By Priority

### P0: Critical Gaps (high-traffic, zero visibility)

1. **Tool Execution Metrics (0/15+ tool handlers instrumented)**
   - `handlers/shell.rs`: Most-frequent tool call; no per-handler count/latency
   - `handlers/apply_patch.rs`: File edits with no success/failure signal
   - `handlers/read_file.rs`, `handlers/grep_files.rs`, `handlers/list_dir.rs`: No counters
   - `handlers/unified_exec.rs`: No counters for exec_command / write_stdin
   - Note: `codex.tool.call` exists at the otel_manager level (aggregated across all tools), but individual tool handler granularity is absent

2. **Error Classification (0/20+ error variants tracked)**
   - `error.rs` defines ~20 distinct error types (Stream, ContextWindowExceeded, RetryLimit, ConnectionFailed, QuotaExceeded, UsageLimitReached, ServerOverloaded, etc.)
   - None are individually counted at creation or call sites
   - Critical for alerting on regressions and quota exhaustion

3. **RegularTask has no counter**
   - Every normal AI turn flows through `tasks/regular.rs`
   - The most common task type yet the only one without a `codex.task.*` counter

### P1: Important Gaps (moderate traffic, useful for debugging)

4. **Cross-platform Sandbox Metrics**
   - Linux Landlock/seccomp: zero metrics
   - macOS seatbelt: zero metrics
   - Only Windows sandbox has instrumentation
   - Missing: `codex.sandbox.exec` (counter, tagged by type), denial/timeout rates

5. **Task Abort/Interrupt Tracking**
   - `tasks/mod.rs::handle_task_abort()` — no counter for task interruptions
   - `GhostSnapshotTask` — no success/failure counter

6. **Protocol & Stream Events**
   - No per-event-type counters for protocol events sent to the client
   - No stream reconnect/parse-error counters in `client.rs`

### P2: Nice-to-Have Gaps (low traffic or dev-only)

7. **TUI Rendering & Input Metrics**
   - Frame rate, render duration, input event counts
   - Slash command usage, clipboard paste size distribution
   - These are primarily useful for performance tuning

8. **CLI Entry Point Metrics**
   - Subcommand invocation counts, exit status tracking
   - Login attempt outcomes
   - Low priority as OtelManager may not be initialized for all CLI paths

9. **Config Loading Metrics**
   - Parse error counters
   - Provider distribution
   - Useful but config loading is typically a one-shot operation

---

## Coverage Score

| Component | Metrics | Status |
|---|---|---|
| Transport/API | 16 | ✅ Strong |
| Memory System | 5 | ✅ Strong |
| Database | 5 | ✅ Strong |
| Session Lifecycle | 4 | ⚠️ Moderate (missing session end, turn retries) |
| Tasks | 4+1 timer | ⚠️ Moderate (missing RegularTask, GhostSnapshot, aborts) |
| MCP | 4 | ⚠️ Moderate (missing per-call duration) |
| Tool Handlers | 0 (individual) | ❌ None (only aggregate `codex.tool.call`) |
| Error Classification | 0 | ❌ None |
| Sandbox (non-Windows) | 0 | ❌ None |
| TUI | 0 | ❌ None |
| CLI | 0 | ❌ None |
| Config | 0 | ❌ None |
| Protocol/Stream | 0 | ❌ None |

**Overall: ~47 metrics covering ~40% of key operational paths. ~60% of components have zero individual metrics.**

---

## Architecture Notes

- All metrics flow through `codex_otel::metrics::global()` → `MetricsClient` → OTEL SDK → OTLP HTTP exporter (Statsig)
- Metric names must follow `codex.*` namespace convention
- Tags are validated via `validation.rs` (alphanumeric + underscore, max 50 chars)
- The `Timer` RAII pattern (records on Drop) is available for easy duration instrumentation
- `RuntimeMetricsSummary` provides in-process metric snapshots for TUI session summary display
