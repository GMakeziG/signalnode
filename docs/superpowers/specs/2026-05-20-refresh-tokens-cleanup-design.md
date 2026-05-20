# Phase 7 — refresh_tokens Cleanup Design

**Date:** 2026-05-20  
**Status:** Approved  
**Scope:** Periodic purge of expired `refresh_tokens` rows in signalnode-core

---

## Problem

Expired rows in `refresh_tokens` accumulate indefinitely. The table is append-heavy (one row per refresh-token rotation) with no automatic eviction. Left unaddressed, this becomes a storage and query-performance issue as the table grows.

---

## Decision

Add a dedicated `purger.rs` module to signalnode-core. It follows the exact same pattern as `worker.rs` and `checker.rs`: a `purge_once` function and a `run_purger` loop, spawned from `main.rs` as a third long-lived Tokio task.

No pg_cron or external scheduler dependency is introduced.

---

## Architecture

### New file: `signalnode-core/src/purger.rs`

```rust
pub async fn purge_once(pool: &PgPool) { ... }
pub async fn run_purger(pool: PgPool, interval: Duration) { ... }
```

- `purge_once` executes one `DELETE` and returns.
- `run_purger` loops: call `purge_once`, sleep `interval`, repeat.

### Changes to `main.rs`

Spawn a third handle alongside the existing two:

```rust
let h3 = tokio::spawn(purger::run_purger(pool.clone(), purge_interval));
let (r1, r2, r3) = tokio::join!(h1, h2, h3);
r3.expect("purger panicked");
```

### Changes to `config.rs`

Add `purge_interval_secs: u64`, read from `TOKEN_PURGE_INTERVAL_SECS`, default `3600`.

---

## SQL

```sql
DELETE FROM refresh_tokens WHERE expires_at < NOW()
```

Single statement, no transaction needed. The `refresh_tokens` table has no dependents — cascades are inbound (from `users`), not outbound.

---

## Error Handling

`purge_once` is non-fatal:

- **Success:** log `rows_deleted` count at `INFO`.
- **DB error:** log at `ERROR`, return. `run_purger` sleeps and retries on the next tick.
- No panic. No retry within the same tick. The loop continues regardless.

---

## Configuration

| Env var | Default | Description |
|---|---|---|
| `TOKEN_PURGE_INTERVAL_SECS` | `3600` | How often the purger runs (seconds) |

---

## Testing

Two integration tests in `purger.rs` under `#[cfg(test)]`, hitting the real DB:

| Test | Setup | Assert |
|---|---|---|
| `purge_once_deletes_expired_tokens` | Insert scratch user + one expired token (`expires_at = NOW() - 1 hour`) + one valid token (`expires_at = NOW() + 1 hour`) | Expired row gone; valid row present |
| `purge_once_no_op_when_nothing_expired` | Insert scratch user + one valid token | Row still present; no error |

Tests clean up after themselves. `#[serial]` added only if DB races surface during the red phase.

---

## Out of Scope

- pg_cron or any Postgres-side scheduler
- Generic janitor abstraction for multiple cleanup jobs
- `login_attempts` cleanup (rows are deleted on successful login; no accumulation problem)
- Admin endpoint to trigger purge on demand
