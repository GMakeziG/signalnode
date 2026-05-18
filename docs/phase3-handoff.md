# Phase 3 Handoff — HTTP Check Execution

**Date:** 2026-05-18  
**Branch:** `main` (pushed to `origin/main`)  
**Tests:** 29 core / 116 API — all green

---

## What Shipped

`signalnode-core` now performs uptime checks autonomously. It spawns two independent Tokio tasks that share one `PgPool` and `reqwest::Client`:

| Task | File | Responsibility |
|---|---|---|
| Delivery worker | `worker.rs` | Claims unsent `pending_notifications`, delivers via webhook/SMTP, stamps `sent_at` |
| HTTP checker | `checker.rs` | Claims due active monitors, fires concurrent GET requests, writes `check_results`, evaluates incidents |

**New migration:** `20260517000011_monitors_last_checked_at.sql`  
Adds `monitors.last_checked_at TIMESTAMPTZ NULL` and partial index `monitors_active_due_idx ON (last_checked_at ASC NULLS FIRST) WHERE status = 'active'`.

**New env var:** `CHECKER_POLL_INTERVAL_SECS` (default `30`, core only).

---

## Key Architectural Decisions

**Two-phase `check_once`**  
Phase 1: short transaction claims due monitors with `FOR UPDATE SKIP LOCKED`, stamps `last_checked_at`, commits immediately. Phase 2: all HTTP checks fire concurrently (no locks held). Each result then gets its own write transaction for error isolation — one failing monitor doesn't roll back others.

**Incident logic duplicated from API**  
`checker.rs` contains a direct port of the incident open/close + notification fanout logic from `signalnode-api/src/check_result/mod.rs`. This was a deliberate Phase 3 decision (see plan comment: "bounded duplication"). Extraction to a shared crate is deferred to Phase 4.

**No API involvement**  
The checker writes `check_results` and opens incidents directly to the database. The API's `POST /check-results` route still exists for manual/external ingestion but is no longer the only path for check data (ADR-0001).

**`Config::from_provider`**  
`Config` now exposes a crate-private `from_provider(get: impl Fn(&str) -> Option<String>)` constructor. All config tests inject values via closure — zero process-wide env mutation. This fixed a latent race between `#[serial]` config tests and `#[sqlx::test]` harness thread startup that became consistent once the checker's sqlx tests were added.

---

## Known Technical Debt

| Item | Severity | Notes |
|---|---|---|
| Incident logic duplicated between `checker.rs` and `signalnode-api/src/check_result/mod.rs` | Medium | Extract to `signalnode-shared` crate in Phase 4. The duplication is bounded and commented. |
| No structured error response bodies (API) | Medium | Errors return bare status codes; clients can't distinguish error types. |
| No refresh token rotation or replay protection | High | Stolen refresh tokens are reusable until 7-day expiry. Requires HITL review. |
| No rate limiting on auth endpoints | High | No protection against credential stuffing. Requires HITL review. |
| `check_membership` / `check_owner` duplicated across three API modules | Low | Extraction deferred post-Phase 3. |
| `build_email_message` re-exported but only used in tests | Low | Remove re-export or expose for future use. |
| Unused imports in `monitor/mod.rs` and `notification_channel/mod.rs` | Low | `delete`, `patch`, `get` — pre-existing warnings. |

---

## Important Implementation Details

**Claim query** (`checker.rs`):
```sql
SELECT ... FROM monitors
WHERE status = 'active'
  AND kind = 'uptime'
  AND (last_checked_at IS NULL
       OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW())
ORDER BY last_checked_at ASC NULLS FIRST
LIMIT 50
FOR UPDATE SKIP LOCKED
```
The `SKIP LOCKED` ensures two concurrent `check_once` calls never double-check the same monitor.

**At-least-once for checks:** `last_checked_at` is stamped in Phase 1 before the HTTP request. If the process dies mid-check, the monitor won't be rechecked until its interval elapses again. Check results are not written on crash — this is intentional (prefer a missed check over a phantom result).

**Incident deduplication:** Application-level guard (`SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1`). No DB-level unique constraint — same as the API (noted in handoff).

**`SmtpConfig` ownership:** Defined in `deliver/email.rs`, re-exported from `deliver/mod.rs`. `config.rs` imports via `crate::deliver`. No separate `config::SmtpConfig`.

---

## Test Status

```
signalnode-core: 29 tests
  config::   7  (unit, no env mutation — use from_provider)
  deliver::  6  (unit, wiremock)
  worker::   5  (sqlx::test — webhook/email delivery)
  checker::  11 (sqlx::test — claim, HTTP check, incidents, loop)

signalnode-api: 116 tests (unchanged from Phase 2)
```

**Test commands:**
```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml

DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

---

## Recommended Next Roadmap Phases

**Priority order:**

1. **Refresh token rotation + replay protection** — security debt items 2–3. Stolen tokens currently have no revocation path before 7-day expiry. Fix: single-use refresh tokens stored in DB, rotated on each `/auth/refresh` call.

2. **Rate limiting on auth endpoints** — security debt item 4. `/auth/login` and `/auth/register` are unprotected against credential stuffing. Fix: `tower_governor` or a Redis-backed counter middleware on Axum.

3. **Structured error response bodies** — security debt item 1. Clients receive bare status codes. Fix: `ProblemDetail` (RFC 9457) response envelope on all error paths.

4. **Extract shared incident logic to `signalnode-shared` crate** — eliminates the bounded duplication between `checker.rs` and `signalnode-api/src/check_result/mod.rs`. Unblocks future monitor kinds without copy-paste.

5. **Additional monitor kinds** — `dns`, `tcp`, `ssl-expiry`. The `kind` column already exists with no DB CHECK constraint, designed to accept new values without a migration.

6. **Webhook payload schema versioning** — add `"version": 1` to the outgoing webhook JSON now while it's a breaking change no one depends on yet.

---

## Suggested First Task for Next Session

**Start with refresh token rotation (roadmap #1).** It's the highest-severity security debt, self-contained (touches only `signalnode-api/src/auth/`), and doesn't require shared infrastructure. A good first step is adding a `refresh_tokens` table migration, then updating `/auth/login` to persist the token hash and `/auth/refresh` to validate-rotate-invalidate in one transaction.

Read `signalnode-api/src/auth/mod.rs` and `migrations/` before starting — the existing JWT flow is complete and the extension point is clear.
