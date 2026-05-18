# HTTP Check Execution (Phase 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add HTTP uptime check execution to `signalnode-core` so it polls active monitors, fires GET requests, writes `check_results` directly to the database, and evaluates incident open/close — without touching the API.

**Architecture:** `main.rs` spawns two independent Tokio tasks sharing one `PgPool` and `reqwest::Client`: the existing notification delivery worker (`worker.rs`) and a new checker loop (`checker.rs`). `check_once` uses two short transactions — a claim phase (stamps `last_checked_at`, commits, releases locks) then concurrent HTTP checks, then a per-monitor write phase (check_result + incident evaluation + notification fanout). Incident logic is duplicated from the API for Phase 3; extraction to a shared crate is deferred to Phase 4.

**Tech Stack:** Rust, sqlx 0.8, reqwest 0.12, tokio 1, futures 0.3, wiremock 0.6 (tests)

**Spec:** `docs/superpowers/specs/2026-05-17-http-check-execution-design.md`

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `migrations/20260517000011_monitors_last_checked_at.sql` | **Create** | Add `last_checked_at` column + partial index to `monitors` |
| `signalnode-core/Cargo.toml` | **Modify** | Add `futures = "0.3"` dependency |
| `signalnode-core/src/config.rs` | **Modify** | Add `checker_poll_interval_secs` field and `CHECKER_POLL_INTERVAL_SECS` env var |
| `signalnode-core/src/checker.rs` | **Create** | `DueMonitor`, `CheckOutcome`, `check_once` (two-phase), `run_checker` |
| `signalnode-core/src/main.rs` | **Modify** | Add `mod checker;`, spawn both tasks with `tokio::join!` |

---

## Task 1: Migration — `last_checked_at` column and partial index

**Files:**
- Create: `migrations/20260517000011_monitors_last_checked_at.sql`

- [ ] **Step 1: Create the migration file**

```sql
ALTER TABLE monitors
    ADD COLUMN last_checked_at TIMESTAMPTZ NULL;

CREATE INDEX monitors_active_due_idx
    ON monitors (last_checked_at ASC NULLS FIRST)
    WHERE status = 'active';
```

Save to `migrations/20260517000011_monitors_last_checked_at.sql`.

- [ ] **Step 2: Verify migration applies cleanly**

Run the existing core test suite — `#[sqlx::test]` applies all migrations automatically:

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: all 16 tests pass, no migration errors.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260517000011_monitors_last_checked_at.sql
git commit -m "feat: add last_checked_at column and partial index to monitors"
```

---

## Task 2: Config — `checker_poll_interval_secs`

**Files:**
- Modify: `signalnode-core/src/config.rs`

- [ ] **Step 1: Write two failing tests**

Add these two tests to the `tests` mod in `signalnode-core/src/config.rs` (inside the existing `mod tests` block, after the existing tests):

```rust
    #[test]
    #[serial]
    fn from_env_uses_checker_interval_default() {
        with_env(|| {
            std::env::set_var("DATABASE_URL", "postgres://unused");
            std::env::remove_var("SMTP_HOST");
            std::env::remove_var("CHECKER_POLL_INTERVAL_SECS");
            let cfg = Config::from_env();
            assert_eq!(cfg.checker_poll_interval_secs, 30);
        });
    }

    #[test]
    #[serial]
    fn from_env_parses_checker_interval() {
        with_env(|| {
            std::env::set_var("DATABASE_URL", "postgres://unused");
            std::env::set_var("CHECKER_POLL_INTERVAL_SECS", "60");
            std::env::remove_var("SMTP_HOST");
            let cfg = Config::from_env();
            assert_eq!(cfg.checker_poll_interval_secs, 60);
            std::env::remove_var("CHECKER_POLL_INTERVAL_SECS");
        });
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml config
```

Expected: FAIL — `no field checker_poll_interval_secs on type Config`

- [ ] **Step 3: Add the field to `Config` and read it in `from_env`**

In `signalnode-core/src/config.rs`, replace the struct definition and `from_env` return:

```rust
pub struct Config {
    pub database_url: String,
    pub smtp: Option<SmtpConfig>,
    pub poll_interval_secs: u64,
    pub checker_poll_interval_secs: u64,
}
```

Inside `Config::from_env`, add after the `poll_interval_secs` block:

```rust
        let checker_poll_interval_secs = std::env::var("CHECKER_POLL_INTERVAL_SECS")
            .ok()
            .map(|v| v.parse::<u64>().expect("CHECKER_POLL_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(30);
```

Update the return value at the bottom of `from_env`:

```rust
        Config { database_url, smtp, poll_interval_secs, checker_poll_interval_secs }
```

- [ ] **Step 4: Run to verify all config tests pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml config
```

Expected: 7 tests pass (5 existing + 2 new).

- [ ] **Step 5: Commit**

```bash
git add signalnode-core/src/config.rs
git commit -m "feat(core): add checker_poll_interval_secs to Config"
```

---

## Task 3: `checker.rs` skeleton — structs, Phase 1 claim, `run_checker` stub

**Files:**
- Modify: `signalnode-core/Cargo.toml`
- Modify: `signalnode-core/src/main.rs`
- Create: `signalnode-core/src/checker.rs`

- [ ] **Step 1: Add `futures` dependency to `signalnode-core/Cargo.toml`**

In the `[dependencies]` section, add after the `reqwest` line:

```toml
futures = "0.3"
```

- [ ] **Step 2: Add `mod checker;` to `main.rs`**

In `signalnode-core/src/main.rs`, add `mod checker;` to the existing module declarations:

```rust
mod checker;
mod config;
mod deliver;
mod worker;
```

- [ ] **Step 3: Write three failing tests in the new `checker.rs`**

Create `signalnode-core/src/checker.rs` with the following content — structs, Phase 1 claim stub (`run_checker` is a real loop; `check_once` implements only Phase 1), and three tests:

```rust
use std::time::{Duration, Instant};

use futures::future::join_all;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct DueMonitor {
    id: Uuid,
    workspace_id: Uuid,
    url: String,
    failure_threshold: i32,
    recovery_threshold: i32,
    interval_secs: i32,
}

struct CheckOutcome {
    monitor: DueMonitor,
    status: &'static str,
    latency_ms: Option<i32>,
    error_detail: Option<String>,
}

pub async fn check_once(pool: &PgPool, _client: &reqwest::Client) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "check_once: failed to begin claim transaction");
            return;
        }
    };

    let monitors = match sqlx::query_as::<_, DueMonitor>(
        "SELECT id, workspace_id, url, failure_threshold, recovery_threshold, interval_secs \
         FROM monitors \
         WHERE status = 'active' \
           AND kind = 'uptime' \
           AND (last_checked_at IS NULL \
                OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW()) \
         ORDER BY last_checked_at ASC NULLS FIRST \
         LIMIT 50 \
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = ?e, "check_once: failed to fetch due monitors");
            return;
        }
    };

    if monitors.is_empty() {
        return;
    }

    for m in &monitors {
        if let Err(e) = sqlx::query("UPDATE monitors SET last_checked_at = NOW() WHERE id = $1")
            .bind(m.id)
            .execute(&mut *tx)
            .await
        {
            tracing::error!(error = ?e, monitor_id = %m.id, "check_once: failed to stamp last_checked_at");
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "check_once: failed to commit claim transaction");
    }
}

pub async fn run_checker(pool: PgPool, client: reqwest::Client, interval: Duration) {
    loop {
        check_once(&pool, &client).await;
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use sqlx::PgPool;

    async fn insert_monitor(pool: &PgPool, url: &str) -> (Uuid, Uuid) {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) \
             VALUES ('checker-test@example.com', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let wid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (name, slug, owner_id) \
             VALUES ('W', 'checker-test', $1) RETURNING id",
        )
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap();

        let mid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO monitors (workspace_id, name, url, interval_secs) \
             VALUES ($1, 'Monitor', $2, 60) RETURNING id",
        )
        .bind(wid)
        .bind(url)
        .fetch_one(pool)
        .await
        .unwrap();

        (wid, mid)
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_skips_paused_monitor(pool: PgPool) {
        let (_wid, mid) = insert_monitor(&pool, "http://127.0.0.1:1").await;
        sqlx::query("UPDATE monitors SET status = 'paused' WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let last_checked_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(last_checked_at.is_none(), "paused monitor should not have last_checked_at stamped");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_skips_not_yet_due_monitor(pool: PgPool) {
        let (_wid, mid) = insert_monitor(&pool, "http://127.0.0.1:1").await;
        // 30s ago with interval_secs=60 → not due for 30 more seconds
        sqlx::query(
            "UPDATE monitors SET last_checked_at = NOW() - INTERVAL '30 seconds' WHERE id = $1",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        let before: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let after: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(before, after, "not-yet-due monitor last_checked_at should be unchanged");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_updates_last_checked_at(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let last_checked_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(last_checked_at.is_some(), "last_checked_at should be stamped after a check cycle");
    }
}
```

- [ ] **Step 4: Run to verify the first two pass, the third fails**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker
```

Expected: `check_once_skips_paused_monitor` PASS, `check_once_skips_not_yet_due_monitor` PASS, `check_once_updates_last_checked_at` FAIL — `last_checked_at` is still `None` (Phase 1 not yet implemented).

> Note: the first two tests pass with the stub because "do nothing" is the correct behavior for negative cases. The third drives the actual Phase 1 implementation.

- [ ] **Step 5: The Phase 1 implementation is already in the file from Step 3** — re-run to confirm all three pass

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker
```

Expected: 3 tests PASS.

> If `check_once_updates_last_checked_at` still fails, verify the claim query has `AND kind = 'uptime'` — the default `kind` for newly inserted monitors is `'uptime'` (see the migration `20260514000005_monitors_crud_fields.sql`).

- [ ] **Step 6: Run full core suite to check nothing broke**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: 21 tests pass (16 existing + 2 config + 3 checker).

- [ ] **Step 7: Commit**

```bash
git add signalnode-core/Cargo.toml signalnode-core/src/main.rs signalnode-core/src/checker.rs
git commit -m "feat(core): add checker.rs skeleton with Phase 1 claim loop"
```

---

## Task 4: `checker.rs` — Phase 2: HTTP check + write `check_result`

**Files:**
- Modify: `signalnode-core/src/checker.rs`

- [ ] **Step 1: Write four failing tests**

Add these four tests to the `mod tests` block in `checker.rs` (after `check_once_updates_last_checked_at`):

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_writes_check_result_for_due_monitor(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "up");
        assert!(row.1.is_some(), "latency_ms should be recorded for 200 response");
        assert!(row.2.is_none(), "error_detail should be None for up");
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_marks_down_on_non_2xx(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "down");
        assert!(row.1.is_some(), "latency_ms should be recorded even for non-2xx");
        assert_eq!(row.2.as_deref(), Some("HTTP 500"));
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_marks_down_on_connect_error(pool: PgPool) {
        let (_wid, mid) = insert_monitor(&pool, "http://127.0.0.1:1").await;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        check_once(&pool, &client).await;

        let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "down");
        assert!(row.1.is_none(), "latency_ms should be None when no response received");
        assert!(row.2.is_some(), "error_detail should contain the error message");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_concurrent_no_duplicate(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let (_wid, _mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        tokio::join!(check_once(&pool, &client), check_once(&pool, &client));

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM check_results")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "FOR UPDATE SKIP LOCKED must prevent duplicate check_results");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker
```

Expected: 3 existing checker tests PASS, 4 new tests FAIL — no `check_results` rows written (Phase 2 not yet implemented).

- [ ] **Step 3: Replace `check_once` with the full two-phase implementation**

Replace the entire `pub async fn check_once` function in `checker.rs` with:

```rust
pub async fn check_once(pool: &PgPool, client: &reqwest::Client) {
    // Phase 1: claim due monitors in a short transaction, stamp last_checked_at immediately
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "check_once: failed to begin claim transaction");
            return;
        }
    };

    let monitors = match sqlx::query_as::<_, DueMonitor>(
        "SELECT id, workspace_id, url, failure_threshold, recovery_threshold, interval_secs \
         FROM monitors \
         WHERE status = 'active' \
           AND kind = 'uptime' \
           AND (last_checked_at IS NULL \
                OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW()) \
         ORDER BY last_checked_at ASC NULLS FIRST \
         LIMIT 50 \
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = ?e, "check_once: failed to fetch due monitors");
            return;
        }
    };

    if monitors.is_empty() {
        return;
    }

    for m in &monitors {
        if let Err(e) = sqlx::query("UPDATE monitors SET last_checked_at = NOW() WHERE id = $1")
            .bind(m.id)
            .execute(&mut *tx)
            .await
        {
            tracing::error!(error = ?e, monitor_id = %m.id, "check_once: failed to stamp last_checked_at");
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "check_once: failed to commit claim transaction");
        return;
    }

    // HTTP checks — no DB locks held; all monitors checked concurrently
    let outcomes: Vec<CheckOutcome> = join_all(monitors.into_iter().map(|m| {
        let client = client.clone();
        async move {
            let start = Instant::now();
            match client.get(&m.url).send().await {
                Ok(resp) if resp.status().is_success() => CheckOutcome {
                    latency_ms: Some(start.elapsed().as_millis() as i32),
                    status: "up",
                    error_detail: None,
                    monitor: m,
                },
                Ok(resp) => CheckOutcome {
                    latency_ms: Some(start.elapsed().as_millis() as i32),
                    status: "down",
                    error_detail: Some(format!("HTTP {}", resp.status().as_u16())),
                    monitor: m,
                },
                Err(e) => CheckOutcome {
                    latency_ms: None,
                    status: "down",
                    error_detail: Some(e.to_string()),
                    monitor: m,
                },
            }
        }
    }))
    .await;

    // Phase 2: write results — one transaction per monitor for error isolation
    for outcome in outcomes {
        let mut tx = match pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to begin write transaction");
                continue;
            }
        };

        if let Err(e) = sqlx::query(
            "INSERT INTO check_results (monitor_id, status, latency_ms, error_detail) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(outcome.monitor.id)
        .bind(outcome.status)
        .bind(outcome.latency_ms)
        .bind(outcome.error_detail.as_deref())
        .execute(&mut *tx)
        .await
        {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to insert check_result");
            continue;
        }

        if let Err(e) = tx.commit().await {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit write transaction");
        } else {
            tracing::info!(monitor_id = %outcome.monitor.id, status = outcome.status, "check result written");
        }
    }
}
```

> Note: incident evaluation is added in Task 5. This version inserts `check_results` and commits without opening/closing incidents.

- [ ] **Step 4: Run to verify all 7 checker tests pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker
```

Expected: 7 tests PASS.

- [ ] **Step 5: Run full core suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: 25 tests pass (16 existing + 2 config + 7 checker).

- [ ] **Step 6: Commit**

```bash
git add signalnode-core/src/checker.rs
git commit -m "feat(core): implement check_once HTTP check and check_result write"
```

---

## Task 5: `checker.rs` — Phase 2: incident evaluation and notification fanout

**Files:**
- Modify: `signalnode-core/src/checker.rs`

- [ ] **Step 1: Write three failing tests**

Add these three tests to the `mod tests` block in `checker.rs` (after `check_once_concurrent_no_duplicate`):

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_opens_incident_on_threshold(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock)
            .await;

        let (wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        sqlx::query("UPDATE monitors SET failure_threshold = 1 WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'webhook', 'https://hooks.example.com/test')",
        )
        .bind(wid)
        .execute(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let incident_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(incident_count, 1, "incident should open when failure_threshold is crossed");

        let pn_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_notifications pn \
             JOIN incidents i ON i.id = pn.incident_id \
             WHERE i.monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pn_count, 1, "one pending_notification per notification_channel");
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_closes_incident_on_recovery(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        sqlx::query("UPDATE monitors SET recovery_threshold = 1 WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let open_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_count, 0, "incident should be closed after recovery");

        let closed_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NOT NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(closed_count, 1);
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_no_duplicate_incident(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        sqlx::query("UPDATE monitors SET failure_threshold = 1 WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "must not open a second incident when one is already open");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker
```

Expected: 7 existing checker tests PASS, 3 new tests FAIL — no incident rows created.

- [ ] **Step 3: Replace the Phase 2 block in `check_once` to add incident evaluation**

In `checker.rs`, replace the entire Phase 2 `for outcome in outcomes` loop with the version below. This adds incident open/close evaluation and notification fanout after the `check_results` insert. The incident logic is a direct port of the API handler at `signalnode-api/src/check_result/mod.rs`:

```rust
    // Phase 2: write results — one transaction per monitor for error isolation
    // Bounded duplication: incident evaluation mirrors signalnode-api/src/check_result/mod.rs
    // Extract to a shared crate in Phase 4.
    for outcome in outcomes {
        let mut tx = match pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to begin write transaction");
                continue;
            }
        };

        if let Err(e) = sqlx::query(
            "INSERT INTO check_results (monitor_id, status, latency_ms, error_detail) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(outcome.monitor.id)
        .bind(outcome.status)
        .bind(outcome.latency_ms)
        .bind(outcome.error_detail.as_deref())
        .execute(&mut *tx)
        .await
        {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to insert check_result");
            continue;
        }

        let open_incident = match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1",
        )
        .bind(outcome.monitor.id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to check open incident");
                if let Err(e) = tx.commit().await {
                    tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit after incident check error");
                }
                continue;
            }
        };

        if open_incident.is_none() {
            let recent = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
            )
            .bind(outcome.monitor.id)
            .bind(outcome.monitor.failure_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to fetch results for open evaluation");
                    if let Err(e) = tx.commit().await {
                        tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit partial write");
                    }
                    continue;
                }
            };

            if recent.len() == outcome.monitor.failure_threshold as usize
                && recent.iter().all(|s| s == "down")
            {
                let incident_id = match sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO incidents (monitor_id) VALUES ($1) RETURNING id",
                )
                .bind(outcome.monitor.id)
                .fetch_one(&mut *tx)
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to open incident");
                        if let Err(e) = tx.commit().await {
                            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit partial write");
                        }
                        continue;
                    }
                };

                let channels = match sqlx::query_as::<_, (String, String)>(
                    "SELECT kind, target FROM notification_channels WHERE workspace_id = $1",
                )
                .bind(outcome.monitor.workspace_id)
                .fetch_all(&mut *tx)
                .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to fetch channels for fanout");
                        if let Err(e) = tx.commit().await {
                            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit partial write");
                        }
                        continue;
                    }
                };

                for (kind, target) in &channels {
                    if let Err(e) = sqlx::query(
                        "INSERT INTO pending_notifications (incident_id, channel_kind, target) \
                         VALUES ($1, $2, $3)",
                    )
                    .bind(incident_id)
                    .bind(kind)
                    .bind(target)
                    .execute(&mut *tx)
                    .await
                    {
                        tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to insert pending notification");
                    }
                }
            }
        } else {
            let recent = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
            )
            .bind(outcome.monitor.id)
            .bind(outcome.monitor.recovery_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to fetch results for close evaluation");
                    if let Err(e) = tx.commit().await {
                        tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit partial write");
                    }
                    continue;
                }
            };

            if recent.len() == outcome.monitor.recovery_threshold as usize
                && recent.iter().all(|s| s == "up")
            {
                if let Err(e) = sqlx::query(
                    "UPDATE incidents SET closed_at = NOW() \
                     WHERE monitor_id = $1 AND closed_at IS NULL",
                )
                .bind(outcome.monitor.id)
                .execute(&mut *tx)
                .await
                {
                    tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to close incident");
                }
            }
        }

        if let Err(e) = tx.commit().await {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit write transaction");
        } else {
            tracing::info!(monitor_id = %outcome.monitor.id, status = outcome.status, "check result written");
        }
    }
```

- [ ] **Step 4: Run to verify all 10 checker tests pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker
```

Expected: 10 tests PASS.

- [ ] **Step 5: Run full core suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: 28 tests pass (16 existing + 2 config + 10 checker).

- [ ] **Step 6: Commit**

```bash
git add signalnode-core/src/checker.rs
git commit -m "feat(core): add incident evaluation and notification fanout to check_once"
```

---

## Task 6: `checker.rs` — `run_checker` loop test

**Files:**
- Modify: `signalnode-core/src/checker.rs`

- [ ] **Step 1: Write the test**

Add this test to the `mod tests` block in `checker.rs` (after `check_once_no_duplicate_incident`):

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn run_checker_loops(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();

        // Tick 1
        check_once(&pool, &client).await;

        // Simulate the monitor's interval having elapsed by backdating last_checked_at
        sqlx::query(
            "UPDATE monitors \
             SET last_checked_at = last_checked_at - INTERVAL '2 minutes' \
             WHERE id = $1",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        // Tick 2
        check_once(&pool, &client).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2, "two check_once ticks should produce two check_results");
    }
```

- [ ] **Step 2: Run to verify it passes**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml checker::tests::run_checker_loops
```

Expected: PASS — `check_once` is already fully implemented; the test passes immediately, confirming that two ticks with an elapsed interval produce two distinct `check_results` rows.

- [ ] **Step 3: Run full core suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: 29 tests pass (16 existing + 2 config + 11 checker).

- [ ] **Step 4: Commit**

```bash
git add signalnode-core/src/checker.rs
git commit -m "test(core): add run_checker_loops test to checker"
```

---

## Task 7: `main.rs` — wire both Tokio tasks

**Files:**
- Modify: `signalnode-core/src/main.rs`

- [ ] **Step 1: Replace `main.rs` with the two-task version**

Replace the entire contents of `signalnode-core/src/main.rs` with:

```rust
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod checker;
mod config;
mod deliver;
mod worker;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");

    let cfg = config::Config::from_env();

    let pool = sqlx::PgPool::connect(&cfg.database_url)
        .await
        .expect("failed to connect to database");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client");

    let worker_interval = Duration::from_secs(cfg.poll_interval_secs);
    let checker_interval = Duration::from_secs(cfg.checker_poll_interval_secs);

    info!(
        worker_interval_secs = cfg.poll_interval_secs,
        checker_interval_secs = cfg.checker_poll_interval_secs,
        smtp_configured = cfg.smtp.is_some(),
        "signalnode-core starting workers"
    );

    let h1 = tokio::spawn(worker::run_worker(
        pool.clone(),
        client.clone(),
        cfg.smtp,
        worker_interval,
    ));
    let h2 = tokio::spawn(checker::run_checker(pool, client, checker_interval));
    let (r1, r2) = tokio::join!(h1, h2);
    r1.expect("delivery worker panicked");
    r2.expect("checker panicked");
}
```

> Both `JoinHandle` results are unwrapped: if either task panics the process exits, and a process supervisor (e.g. Docker restart policy) is expected to restart it.

- [ ] **Step 2: Verify the binary compiles**

```bash
cargo build --manifest-path signalnode-core/Cargo.toml
```

Expected: compiles with no errors. Warnings about `build_email_message` and unused imports in `signalnode-api` are pre-existing and non-blocking.

- [ ] **Step 3: Run the full core test suite one more time to confirm nothing regressed**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: 29 tests pass.

- [ ] **Step 4: Commit**

```bash
git add signalnode-core/src/main.rs
git commit -m "feat(core): wire checker and delivery worker as independent Tokio tasks"
```

---

## Task 8: Final verification

**Files:** none

- [ ] **Step 1: Run the full `signalnode-core` test suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected output (order may vary):

```
running 29 tests
test config::tests::from_env_panics_without_database_url ... ok
test config::tests::from_env_parses_poll_interval ... ok
test config::tests::from_env_uses_poll_interval_default ... ok
test config::tests::from_env_smtp_none_when_no_host ... ok
test config::tests::from_env_smtp_some_with_all_vars ... ok
test config::tests::from_env_uses_checker_interval_default ... ok
test config::tests::from_env_parses_checker_interval ... ok
test worker::tests::poll_once_delivers_webhook_and_marks_sent ... ok
test worker::tests::poll_once_leaves_row_on_delivery_failure ... ok
test worker::tests::poll_once_skips_email_when_smtp_not_configured ... ok
test worker::tests::poll_once_skips_already_sent_rows ... ok
test worker::tests::poll_once_sends_correct_json_payload ... ok
... (remaining 5 worker tests)
test checker::tests::check_once_skips_paused_monitor ... ok
test checker::tests::check_once_skips_not_yet_due_monitor ... ok
test checker::tests::check_once_updates_last_checked_at ... ok
test checker::tests::check_once_writes_check_result_for_due_monitor ... ok
test checker::tests::check_once_marks_down_on_non_2xx ... ok
test checker::tests::check_once_marks_down_on_connect_error ... ok
test checker::tests::check_once_concurrent_no_duplicate ... ok
test checker::tests::check_once_opens_incident_on_threshold ... ok
test checker::tests::check_once_closes_incident_on_recovery ... ok
test checker::tests::check_once_no_duplicate_incident ... ok
test checker::tests::run_checker_loops ... ok

test result: ok. 29 passed; 0 failed
```

- [ ] **Step 2: Run the full `signalnode-api` test suite to confirm no regressions**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: 116 tests pass (unchanged from Phase 2).

- [ ] **Step 3: Verify acceptance criteria from the spec**

Confirm each of the 7 acceptance criteria is met:

1. ✅ Active `uptime` monitors receive a `check_results` row every `interval_secs` seconds (verified by `run_checker_loops`)
2. ✅ Incidents open automatically when `failure_threshold` is crossed (verified by `check_once_opens_incident_on_threshold`)
3. ✅ `pending_notifications` rows created for each channel when an incident opens (same test)
4. ✅ Open incidents close automatically when `recovery_threshold` is crossed (verified by `check_once_closes_incident_on_recovery`)
5. ✅ `monitors.last_checked_at` updated after every check cycle (verified by `check_once_updates_last_checked_at`)
6. ✅ Paused/archived monitors never checked (verified by `check_once_skips_paused_monitor`; archived is filtered by same `status = 'active'` clause)
7. ✅ All 29 core tests and 116 API tests pass

- [ ] **Step 4: Update the session handoff memory**

Update `docs/superpowers/` or the memory file to reflect Phase 3 complete status, new test counts (29 core / 116 api), and the new env var (`CHECKER_POLL_INTERVAL_SECS`, default 30).
