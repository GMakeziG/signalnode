# HTTP Check Execution — Design Spec

**Date:** 2026-05-17
**Phase:** 3
**Status:** Approved

## Acceptance Criteria

Phase 3 is complete when, with no API calls after initial setup:

1. A monitor with `status = 'active'` and `kind = 'uptime'` receives a `check_result` row approximately every `interval_secs` seconds, written directly by `signalnode-core`.
2. An incident opens automatically once `failure_threshold` consecutive `"down"` results are recorded.
3. A `pending_notifications` row is created for each workspace `notification_channel` when an incident opens; the delivery worker delivers it on its next poll.
4. An open incident closes automatically once `recovery_threshold` consecutive `"up"` results are recorded.
5. `monitors.last_checked_at` is updated after every check cycle.
6. A paused or archived monitor is never checked by the engine.
7. All 11 `checker.rs` tests pass; existing 132 tests continue to pass.

## Overview

`signalnode-core` currently delivers notifications from the `pending_notifications` outbox. This spec adds the other half of its ADR-0001 charter: polling active Monitors from the database, executing HTTP uptime checks, writing `check_results` directly, and evaluating incident open/close — all without touching the API.

## Architecture

`main.rs` spawns two independent Tokio tasks after building the shared `PgPool` and `reqwest::Client` (both `Arc`-backed, cheap to clone):

```
tokio::spawn(worker::run_worker(pool.clone(), client.clone(), smtp, worker_interval))
tokio::spawn(checker::run_checker(pool, client, checker_interval))
```

Each loop runs at its own configurable interval:

| Env var | Default | Purpose |
|---|---|---|
| `WORKER_POLL_INTERVAL_SECS` | 10 | Notification delivery cadence |
| `CHECKER_POLL_INTERVAL_SECS` | 30 | Monitor check cadence |

`main.rs` awaits both handles with `tokio::join!` and unwraps the `JoinHandle` results — if either task panics, the unwrap propagates the panic and the process exits. A process supervisor (e.g. Docker restart policy) is expected to restart it. A new `signalnode-core/src/checker.rs` module mirrors the structure of `worker.rs`. The notification delivery worker (`worker.rs`) is unchanged.

**Bounded duplication:** The incident evaluation and notification fanout logic is duplicated from the API handler into `checker.rs`. This is intentional for Phase 3. A shared-crate extraction is deferred to after Phase 3 is working end-to-end.

## Database Migration

One migration (`20260517000011_monitors_last_checked_at.sql`):

```sql
ALTER TABLE monitors
    ADD COLUMN last_checked_at TIMESTAMPTZ NULL;

CREATE INDEX monitors_active_due_idx
    ON monitors (last_checked_at ASC NULLS FIRST)
    WHERE status = 'active';
```

`NULL` means the monitor has never been checked — treated as immediately due. The partial index covers only `active` monitors and orders by `last_checked_at`, keeping the checker's claim query on a tight index scan as monitor count grows.

## `checker.rs` — Two-Phase Check Loop

### Why two phases

HTTP calls must not hold database row locks. A single transaction that claims monitors, fires HTTP requests, and writes results would hold `FOR UPDATE` locks for up to `50 × timeout` seconds. Instead, `check_once` uses two short transactions separated by lock-free network I/O.

### Phase 1 — Claim

Short transaction:

```sql
SELECT id, workspace_id, url, failure_threshold, recovery_threshold, interval_secs
FROM monitors
WHERE status = 'active'
  AND kind = 'uptime'
  AND (last_checked_at IS NULL
       OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW())
ORDER BY last_checked_at ASC NULLS FIRST
LIMIT 50
FOR UPDATE SKIP LOCKED
```

For each claimed monitor, stamp `last_checked_at = NOW()` immediately, then commit. Rows are now "claimed" — they won't be picked up by another worker instance until `interval_secs` elapses.

### HTTP Check (no locks held)

For each claimed monitor, concurrently via `tokio::join_all` (no concurrency cap for Phase 3 — at most 50 simultaneous connections given the LIMIT):


- `GET {url}` using the shared `reqwest::Client` (10 s timeout set in `main.rs`)
- Measure wall-clock latency with `std::time::Instant`
- Classify:
  - 2xx response → `"up"`, `latency_ms` = elapsed ms
  - Non-2xx response → `"down"`, `error_detail` = `"HTTP {status_code}"`, `latency_ms` = elapsed ms
  - Timeout / connect error → `"down"`, `error_detail` = error message string, `latency_ms` = `None`

### Phase 2 — Write Results

One transaction **per monitor** (not one batch transaction — per-monitor transactions preserve the error isolation guarantee: a DB failure for one monitor never rolls back another's result):

1. `INSERT INTO check_results (monitor_id, status, latency_ms, error_detail) VALUES ...`
2. Evaluate incident open/close (threshold logic — see below)
3. If a new incident opens: `INSERT INTO pending_notifications` (one row per workspace `notification_channel`)
4. Commit

The delivery worker picks up `pending_notifications` rows on its next poll.

### Incident Evaluation

Duplicated from `signalnode-api/src/check_result/mod.rs`. Only runs when `monitors.status = 'active'` (already guaranteed by the claim query).

- **Open:** No open incident + last N `check_results` are all `"down"` (N = `failure_threshold`) → `INSERT INTO incidents`
- **Close:** Open incident exists + last N `check_results` are all `"up"` (N = `recovery_threshold`) → `UPDATE incidents SET closed_at = NOW()`

## Error Handling

Failures are isolated per monitor — one bad check never aborts the batch.

| Failure | Behaviour |
|---|---|
| DB error during Phase 1 (claim) | `tracing::error!`, return early; monitors remain unclaimed and retry next tick |
| HTTP error (timeout, connect, DNS) | Status = `"down"`, `error_detail` = error string, continue to write phase |
| Non-2xx HTTP response | Status = `"down"`, `error_detail` = `"HTTP {code}"`, continue to write phase |
| DB error during Phase 2 (write) | `tracing::error!` per monitor, skip that monitor's result write |
| DB error during incident evaluation | `tracing::error!`, skip incident open/close for that monitor; check result is still written |

**Known Phase 3 debt:** If Phase 2 fails after Phase 1 has already stamped `last_checked_at`, that monitor skips one `interval_secs` window without a recorded `CheckResult`. This is acceptable for Phase 3 — the poll loop is the retry mechanism.

## Module Layout (post-Phase 3)

```
signalnode-core/src/
  main.rs       — startup: env → pool + client → tokio::join!(run_worker, run_checker)
  config.rs     — Config: adds checker_poll_interval_secs
  deliver/
    mod.rs
    webhook.rs
    email.rs
  worker.rs     — PendingRow, poll_once, run_worker (unchanged)
  checker.rs    — DueMonitor, check_once, run_checker (new)
```

## Config Changes

`Config` gains one field:

```rust
pub checker_poll_interval_secs: u64,  // default 30, from CHECKER_POLL_INTERVAL_SECS
```

`Config::from_env` reads it with the same pattern as `poll_interval_secs`.

## Tests

All tests use `#[sqlx::test]` with real Postgres and `wiremock` for HTTP mocking, matching the `worker.rs` pattern.

| Test | What it verifies |
|---|---|
| `check_once_writes_check_result_for_due_monitor` | Mock returns 200; `check_results` row inserted with status `"up"` and latency |
| `check_once_marks_down_on_non_2xx` | Mock returns 500; status = `"down"`, `error_detail` = `"HTTP 500"` |
| `check_once_marks_down_on_connect_error` | Bad URL (port 1); status = `"down"`, `error_detail` non-empty |
| `check_once_skips_paused_monitor` | Paused monitor not claimed; no `check_results` row written |
| `check_once_skips_not_yet_due_monitor` | `last_checked_at` = NOW() in fixture; monitor not claimed |
| `check_once_updates_last_checked_at` | After run, `monitors.last_checked_at` is non-null |
| `check_once_opens_incident_on_threshold` | `failure_threshold = 1`, mock returns 500; incident opens, `pending_notifications` row created |
| `check_once_closes_incident_on_recovery` | Pre-inserted open incident, mock returns 200, `recovery_threshold = 1`; incident closed |
| `check_once_no_duplicate_incident` | Open incident already exists; second down check does not open a second incident |
| `check_once_concurrent_no_duplicate` | Two concurrent `check_once` calls; `FOR UPDATE SKIP LOCKED` ensures exactly one `check_results` row per monitor |
| `run_checker_loops` | Two ticks with a short interval; two `check_results` rows written for the same monitor |
