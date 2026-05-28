# Phase 12: TCP Port Checks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add TCP port monitoring as a second check kind alongside the existing HTTP uptime checker, end-to-end from schema to `signalnode-core` execution.

**Architecture:** Option A — single `check_once` branching on `monitors.kind`. One new migration makes `url` nullable and adds `tcp_host`/`tcp_port`. The API validates kind-specific fields on create. The checker queries both kinds and dispatches to a new `check_tcp` function. Incident evaluation is unchanged.

**Tech Stack:** Rust, sqlx 0.8 (Postgres), Axum, Tokio (`TcpStream::connect`, `time::timeout`), reqwest.

---

## File Map

| File | Action | What changes |
|---|---|---|
| `migrations/20260528000014_monitors_tcp_fields.sql` | **CREATE** | `url` nullable, add `tcp_host`/`tcp_port`, `monitors_target_check` constraint |
| `signalnode-api/src/monitor/mod.rs` | **MODIFY** | `Monitor` + `CreateMonitorRequest` structs, validation, INSERT + all SELECT lists |
| `signalnode-core/src/config.rs` | **MODIFY** | `tcp_check_timeout_ms` field + `TCP_CHECK_TIMEOUT_MS` env var |
| `signalnode-core/src/checker.rs` | **MODIFY** | `DueMonitor`, claim query, `check_tcp` fn, `check_once`/`run_checker` signatures, tests |
| `signalnode-core/src/main.rs` | **MODIFY** | Pass `tcp_timeout` to `run_checker` |

---

## Task 1: Schema Migration

**Files:**
- Create: `migrations/20260528000014_monitors_tcp_fields.sql`

- [ ] **Step 1: Write the migration**

Create `migrations/20260528000014_monitors_tcp_fields.sql`:

```sql
ALTER TABLE monitors
    ALTER COLUMN url DROP NOT NULL,
    ADD COLUMN tcp_host TEXT,
    ADD COLUMN tcp_port INT
        CHECK (tcp_port IS NULL OR (tcp_port >= 1 AND tcp_port <= 65535)),
    ADD CONSTRAINT monitors_target_check CHECK (
        (kind = 'uptime' AND url IS NOT NULL)
        OR (kind = 'tcp' AND tcp_host IS NOT NULL AND tcp_port IS NOT NULL)
    );
```

There are no existing named CHECK constraints on `url` or `kind`, so nothing needs to be dropped first. Existing `kind = 'uptime'` rows all have `url IS NOT NULL` — the new constraint passes without a backfill.

- [ ] **Step 2: Verify migration applies cleanly**

```bash
cd /home/ninjatronics/src/signalnode
DATABASE_URL=postgres://localhost/signalnode_test cargo test --test '*' -p signalnode-api -- --test-threads=1 2>&1 | tail -5
```

Expected: all existing tests pass (migrations are applied per-test by `sqlx::test`). If there are failures unrelated to this task, investigate before continuing.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260528000014_monitors_tcp_fields.sql
git commit -m "feat(db): add tcp_host/tcp_port columns, make url nullable (Phase 12)"
```

---

## Task 2: Config — TCP timeout env var

**Files:**
- Modify: `signalnode-core/src/config.rs`

- [ ] **Step 1: Write two failing config unit tests**

Add to the `#[cfg(test)]` block in `signalnode-core/src/config.rs`, inside the existing `mod tests { ... }`:

```rust
#[test]
fn from_env_uses_tcp_check_timeout_default() {
    let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
    assert_eq!(cfg.tcp_check_timeout_ms, 5000);
}

#[test]
fn from_env_parses_tcp_check_timeout() {
    let cfg = Config::from_provider(vars(&[
        ("DATABASE_URL", "postgres://unused"),
        ("TCP_CHECK_TIMEOUT_MS", "3000"),
    ]));
    assert_eq!(cfg.tcp_check_timeout_ms, 3000);
}
```

- [ ] **Step 2: Run the new tests to confirm they fail**

```bash
cd /home/ninjatronics/src/signalnode
cargo test -p signalnode-core config 2>&1 | grep -E "FAILED|error"
```

Expected: compile error — `tcp_check_timeout_ms` does not exist on `Config`.

- [ ] **Step 3: Add `tcp_check_timeout_ms` to the `Config` struct and parse it**

In `signalnode-core/src/config.rs`, replace the struct and `from_provider` to match:

```rust
pub struct Config {
    pub database_url: String,
    pub smtp: Option<SmtpConfig>,
    pub poll_interval_secs: u64,
    pub checker_poll_interval_secs: u64,
    pub purge_interval_secs: u64,
    pub tcp_check_timeout_ms: u64,
}
```

Inside `from_provider`, add after the `purge_interval_secs` binding (before the final `Config { ... }` return):

```rust
let tcp_check_timeout_ms = get("TCP_CHECK_TIMEOUT_MS")
    .map(|v| v.parse::<u64>().expect("TCP_CHECK_TIMEOUT_MS must be a positive integer"))
    .unwrap_or(5000);
```

Update the final `Config { ... }` constructor to include the new field:

```rust
Config {
    database_url,
    smtp,
    poll_interval_secs,
    checker_poll_interval_secs,
    purge_interval_secs,
    tcp_check_timeout_ms,
}
```

- [ ] **Step 4: Run the new tests to confirm they pass**

```bash
cd /home/ninjatronics/src/signalnode
cargo test -p signalnode-core config 2>&1 | grep -E "test .* ok|FAILED"
```

Expected: all config tests pass, including the two new ones.

- [ ] **Step 5: Commit**

```bash
git add signalnode-core/src/config.rs
git commit -m "feat(core): add TCP_CHECK_TIMEOUT_MS config (Phase 12)"
```

---

## Task 3: API — structs, validation, SQL, tests

**Files:**
- Modify: `signalnode-api/src/monitor/mod.rs`

- [ ] **Step 1: Write four failing API tests**

Add these four tests to the `#[cfg(test)] mod tests` block at the bottom of `signalnode-api/src/monitor/mod.rs`:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_tcp_monitor_success(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors"),
        uid,
        Some(json!({
            "name": "DB Port",
            "kind": "tcp",
            "tcp_host": "127.0.0.1",
            "tcp_port": 5432,
            "interval_secs": 60
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["kind"], "tcp");
    assert_eq!(json["tcp_host"], "127.0.0.1");
    assert_eq!(json["tcp_port"], 5432);
    assert!(json["url"].is_null(), "url should be null for tcp monitors");
}

#[sqlx::test(migrations = "../migrations")]
async fn create_tcp_monitor_missing_fields(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    // missing tcp_host
    let res = authed(
        pool.clone(),
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors"),
        uid,
        Some(json!({"name": "DB Port", "kind": "tcp", "tcp_port": 5432, "interval_secs": 60})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");

    // missing tcp_port
    let res2 = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors"),
        uid,
        Some(json!({"name": "DB Port", "kind": "tcp", "tcp_host": "127.0.0.1", "interval_secs": 60})),
    )
    .await;
    assert_eq!(res2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../migrations")]
async fn create_tcp_monitor_invalid_port(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    for port in &[0_i64, 65536_i64] {
        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            Some(json!({
                "name": "DB Port",
                "kind": "tcp",
                "tcp_host": "127.0.0.1",
                "tcp_port": port,
                "interval_secs": 60
            })),
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "port {port} should be rejected"
        );
    }
}

#[sqlx::test(migrations = "../migrations")]
async fn create_tcp_monitor_rejects_empty_host(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors"),
        uid,
        Some(json!({
            "name": "DB Port",
            "kind": "tcp",
            "tcp_host": "",
            "tcp_port": 5432,
            "interval_secs": 60
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 2: Run the new tests to confirm they fail**

```bash
cd /home/ninjatronics/src/signalnode
cargo test -p signalnode-api create_tcp_monitor 2>&1 | grep -E "FAILED|error\[" | head -10
```

Expected: compile error or runtime FAILED — `tcp_host`, `tcp_port`, `kind` fields not on `CreateMonitorRequest`.

- [ ] **Step 3: Update the `Monitor` struct**

Replace the `Monitor` struct (lines 18–31) with:

```rust
#[derive(Serialize, sqlx::FromRow)]
struct Monitor {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    url: Option<String>,
    interval_secs: i32,
    status: String,
    failure_threshold: i32,
    recovery_threshold: i32,
    kind: String,
    tcp_host: Option<String>,
    tcp_port: Option<i32>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Update the `CreateMonitorRequest` struct**

Replace the `CreateMonitorRequest` struct (lines 33–40) with:

```rust
#[derive(Deserialize)]
struct CreateMonitorRequest {
    name: String,
    kind: Option<String>,
    url: Option<String>,
    interval_secs: i32,
    failure_threshold: Option<i32>,
    recovery_threshold: Option<i32>,
    tcp_host: Option<String>,
    tcp_port: Option<i32>,
}
```

- [ ] **Step 5: Replace the validation + INSERT block in `create_monitor`**

Replace everything from `let failure_threshold = ...` through the closing `}` of the `match sqlx::query_as` call (lines 79–114) with:

```rust
    let failure_threshold = body.failure_threshold.unwrap_or(1);
    let recovery_threshold = body.recovery_threshold.unwrap_or(1);

    if body.name.is_empty() || body.interval_secs < 1 || failure_threshold < 1 || recovery_threshold < 1 {
        return MonitorError::InvalidInput(
            "Name must not be empty; interval, failure and recovery thresholds must be >= 1".into(),
        )
        .into_response();
    }

    let kind = body.kind.as_deref().unwrap_or("uptime");
    let (url_val, tcp_host_val, tcp_port_val) = match kind {
        "uptime" => {
            if body.url.as_deref().map(|u| u.is_empty()).unwrap_or(true) {
                return MonitorError::InvalidInput(
                    "uptime monitors require a non-empty url".into(),
                )
                .into_response();
            }
            (body.url.as_deref(), None::<&str>, None::<i32>)
        }
        "tcp" => {
            if body.tcp_host.as_deref().map(|h| h.is_empty()).unwrap_or(true) {
                return MonitorError::InvalidInput(
                    "tcp monitors require a non-empty tcp_host".into(),
                )
                .into_response();
            }
            let port = body.tcp_port.unwrap_or(0);
            if port < 1 || port > 65535 {
                return MonitorError::InvalidInput(
                    "tcp monitors require tcp_port between 1 and 65535".into(),
                )
                .into_response();
            }
            (None::<&str>, body.tcp_host.as_deref(), Some(port))
        }
        _ => {
            return MonitorError::InvalidInput("kind must be 'uptime' or 'tcp'".into())
                .into_response();
        }
    };

    match sqlx::query_as::<_, Monitor>(
        "INSERT INTO monitors \
             (workspace_id, name, kind, url, tcp_host, tcp_port, interval_secs, failure_threshold, recovery_threshold) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         RETURNING id, workspace_id, name, url, interval_secs, status, \
                   failure_threshold, recovery_threshold, kind, tcp_host, tcp_port, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(&body.name)
    .bind(kind)
    .bind(url_val)
    .bind(tcp_host_val)
    .bind(tcp_port_val)
    .bind(body.interval_secs)
    .bind(failure_threshold)
    .bind(recovery_threshold)
    .fetch_one(&state.pool)
    .await
    {
        Ok(monitor) => (StatusCode::CREATED, Json(monitor)).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert monitor");
            MonitorError::Internal.into_response()
        }
    }
```

- [ ] **Step 6: Update all SELECT column lists to include `tcp_host, tcp_port`**

Five SQL strings need `tcp_host, tcp_port` added to their SELECT column list. Make these changes:

**`list_monitors` — first SQL string (include_archived = false path):**

```rust
"SELECT id, workspace_id, name, url, interval_secs, status,
        failure_threshold, recovery_threshold, kind, tcp_host, tcp_port, created_at, updated_at
 FROM monitors WHERE workspace_id = $1 AND status != 'archived' ORDER BY created_at ASC"
```

**`list_monitors` — second SQL string (include_archived = true path):**

```rust
"SELECT id, workspace_id, name, url, interval_secs, status,
        failure_threshold, recovery_threshold, kind, tcp_host, tcp_port, created_at, updated_at
 FROM monitors WHERE workspace_id = $1 ORDER BY created_at ASC"
```

**`get_monitor`:**

```rust
"SELECT id, workspace_id, name, url, interval_secs, status,
        failure_threshold, recovery_threshold, kind, tcp_host, tcp_port, created_at, updated_at
 FROM monitors WHERE id = $1 AND workspace_id = $2"
```

**`patch_monitor` — the `SELECT status` query is unchanged (scalar). The `UPDATE … RETURNING` query:**

```rust
"UPDATE monitors
 SET name               = COALESCE($1, name),
     url                = COALESCE($2, url),
     interval_secs      = COALESCE($3, interval_secs),
     status             = COALESCE($4, status),
     failure_threshold  = COALESCE($5, failure_threshold),
     recovery_threshold = COALESCE($6, recovery_threshold),
     updated_at         = NOW()
 WHERE id = $7 AND workspace_id = $8
 RETURNING id, workspace_id, name, url, interval_secs, status,
           failure_threshold, recovery_threshold, kind, tcp_host, tcp_port, created_at, updated_at"
```

- [ ] **Step 7: Run the full API test suite**

```bash
cd /home/ninjatronics/src/signalnode
cargo test -p signalnode-api 2>&1 | tail -15
```

Expected: all tests pass including the four new TCP tests. The existing `create_monitor_success`, `create_monitor_invalid_body`, and all other monitor tests must still be green — their JSON bodies have no `kind` field, so validation defaults to `"uptime"` and the url check runs unchanged.

- [ ] **Step 8: Commit**

```bash
git add signalnode-api/src/monitor/mod.rs
git commit -m "feat(api): support tcp monitor kind with host/port validation (Phase 12)"
```

---

## Task 4: Checker — DueMonitor, query, check_tcp, signatures, tests

**Files:**
- Modify: `signalnode-core/src/checker.rs`
- Modify: `signalnode-core/src/main.rs`

- [ ] **Step 1: Add `tokio::net::TcpStream` import and update `DueMonitor`**

In `signalnode-core/src/checker.rs`, replace the existing imports and `DueMonitor` struct (lines 1–15):

```rust
use std::time::{Duration, Instant};

use futures::future::join_all;
use sqlx::PgPool;
use tokio::net::TcpStream;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct DueMonitor {
    id: Uuid,
    workspace_id: Uuid,
    url: Option<String>,
    failure_threshold: i32,
    recovery_threshold: i32,
    interval_secs: i32,
    kind: String,
    tcp_host: Option<String>,
    tcp_port: Option<i32>,
}
```

- [ ] **Step 2: Add the `check_tcp` function**

Add this new function immediately before `pub async fn check_once(...)`:

```rust
async fn check_tcp(host: &str, port: u16, timeout_dur: Duration, monitor: DueMonitor) -> CheckOutcome {
    let addr = format!("{host}:{port}");
    let start = Instant::now();
    match tokio::time::timeout(timeout_dur, TcpStream::connect(&*addr)).await {
        Ok(Ok(stream)) => {
            drop(stream);
            CheckOutcome {
                latency_ms: Some(start.elapsed().as_millis() as i32),
                status: "up",
                error_detail: None,
                monitor,
            }
        }
        Ok(Err(e)) => CheckOutcome {
            latency_ms: None,
            status: "down",
            error_detail: Some(e.to_string()),
            monitor,
        },
        Err(_) => CheckOutcome {
            latency_ms: None,
            status: "down",
            error_detail: Some(format!(
                "connection timed out after {}ms",
                timeout_dur.as_millis()
            )),
            monitor,
        },
    }
}
```

- [ ] **Step 3: Update `check_once` signature and claim query**

Replace the `check_once` signature (line 24) with:

```rust
pub async fn check_once(pool: &PgPool, client: &reqwest::Client, tcp_timeout: Duration) {
```

Replace the claim query string inside `sqlx::query_as::<_, DueMonitor>(...)` with:

```rust
"SELECT id, workspace_id, url, failure_threshold, recovery_threshold, interval_secs, \
         kind, tcp_host, tcp_port \
 FROM monitors \
 WHERE status = 'active' \
   AND kind IN ('uptime', 'tcp') \
   AND (last_checked_at IS NULL \
        OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW()) \
 ORDER BY last_checked_at ASC NULLS FIRST \
 LIMIT 50 \
 FOR UPDATE SKIP LOCKED"
```

- [ ] **Step 4: Replace the concurrent check loop**

Replace the entire `// HTTP checks — no DB locks held...` section (lines 74–101) with:

```rust
    // Checks — no DB locks held; all monitors checked concurrently
    let outcomes: Vec<CheckOutcome> = join_all(monitors.into_iter().map(|m| {
        let client = client.clone();
        async move {
            match m.kind.as_str() {
                "uptime" => {
                    let url = m.url.as_deref().unwrap_or("").to_string();
                    let start = Instant::now();
                    match client.get(&url).send().await {
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
                "tcp" => {
                    let host = m.tcp_host.as_deref().unwrap_or("").to_string();
                    let port = m.tcp_port.unwrap_or(0) as u16;
                    check_tcp(&host, port, tcp_timeout, m).await
                }
                _ => {
                    let kind = m.kind.clone();
                    tracing::error!(monitor_id = %m.id, kind = %kind, "check_once: unknown monitor kind");
                    // isolated arm — add metric counter here during observability phase
                    CheckOutcome {
                        status: "down",
                        latency_ms: None,
                        error_detail: Some(format!("unknown monitor kind: {kind}")),
                        monitor: m,
                    }
                }
            }
        }
    }))
    .await;
```

- [ ] **Step 5: Update `run_checker` signature**

Replace the `run_checker` function signature and body:

```rust
pub async fn run_checker(pool: PgPool, client: reqwest::Client, interval: Duration, tcp_timeout: Duration) {
    loop {
        check_once(&pool, &client, tcp_timeout).await;
        tokio::time::sleep(interval).await;
    }
}
```

- [ ] **Step 6: Wire `tcp_timeout` through `main.rs`**

In `signalnode-core/src/main.rs`, add after `let purge_interval = ...`:

```rust
let tcp_timeout = Duration::from_millis(cfg.tcp_check_timeout_ms);
```

Replace the `h2` spawn line with:

```rust
let h2 = tokio::spawn(checker::run_checker(pool.clone(), client, checker_interval, tcp_timeout));
```

- [ ] **Step 7: Write the two new TCP checker tests and helper**

Add `insert_tcp_monitor` helper and both tests to the `#[cfg(test)] mod tests` block in `checker.rs`. Add them after the existing `insert_monitor` function:

```rust
async fn insert_tcp_monitor(pool: &PgPool, host: &str, port: i32) -> (Uuid, Uuid) {
    let uid = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (email, password_hash) \
         VALUES ('tcp-checker-test@example.com', 'x') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();

    let wid = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO workspaces (name, slug, owner_id) \
         VALUES ('W', 'tcp-checker-test', $1) RETURNING id",
    )
    .bind(uid)
    .fetch_one(pool)
    .await
    .unwrap();

    let mid = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO monitors (workspace_id, name, kind, tcp_host, tcp_port, interval_secs) \
         VALUES ($1, 'TCP Monitor', 'tcp', $2, $3, 60) RETURNING id",
    )
    .bind(wid)
    .bind(host)
    .bind(port)
    .fetch_one(pool)
    .await
    .unwrap();

    (wid, mid)
}

#[sqlx::test(migrations = "../migrations")]
async fn check_once_tcp_up_when_port_open(pool: PgPool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port() as i32;

    let (_wid, mid) = insert_tcp_monitor(&pool, "127.0.0.1", port).await;

    let client = reqwest::Client::new();
    check_once(&pool, &client, Duration::from_millis(5000)).await;

    let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
        "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
    )
    .bind(mid)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, "up");
    assert!(row.1.is_some(), "latency_ms should be recorded for successful TCP connect");
    assert!(row.2.is_none(), "error_detail should be None for up");

    drop(listener);
}

#[sqlx::test(migrations = "../migrations")]
async fn check_once_tcp_down_when_port_refused(pool: PgPool) {
    let (_wid, mid) = insert_tcp_monitor(&pool, "127.0.0.1", 1).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    check_once(&pool, &client, Duration::from_millis(200)).await;

    let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
        "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
    )
    .bind(mid)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, "down");
    assert!(row.1.is_none(), "latency_ms should be None when connection refused");
    assert!(row.2.is_some(), "error_detail should contain the error message");
}
```

- [ ] **Step 8: Update all existing `check_once` call sites in the tests module**

Every existing `check_once(&pool, &client).await` call in the `mod tests` block must become `check_once(&pool, &client, Duration::from_millis(5000)).await`. There are eleven such calls across these tests: `check_once_skips_paused_monitor`, `check_once_skips_not_yet_due_monitor`, `check_once_updates_last_checked_at`, `check_once_writes_check_result_for_due_monitor`, `check_once_marks_down_on_non_2xx`, `check_once_marks_down_on_connect_error`, both calls in `check_once_concurrent_no_duplicate` (inside `tokio::join!`), `check_once_opens_incident_on_threshold`, `check_once_closes_incident_on_recovery`, `check_once_no_duplicate_incident`, and both calls in `run_checker_loops`.

The `tokio::join!` call in `check_once_concurrent_no_duplicate` becomes:

```rust
tokio::join!(
    check_once(&pool, &client, Duration::from_millis(5000)),
    check_once(&pool, &client, Duration::from_millis(5000))
);
```

- [ ] **Step 9: Run the full test suite for both crates**

```bash
cd /home/ninjatronics/src/signalnode
cargo test -p signalnode-core 2>&1 | tail -20
cargo test -p signalnode-api 2>&1 | tail -10
```

Expected: all tests in both crates pass, including the two new TCP checker tests and the four new API monitor tests. Total should be at or above 189 tests (183 existing + 6 new).

- [ ] **Step 10: Commit**

```bash
git add signalnode-core/src/checker.rs signalnode-core/src/main.rs
git commit -m "feat(core): add TCP port check execution in checker (Phase 12)"
```

---

## Self-Review Checklist (run before marking complete)

- [ ] All `check_once` calls in `checker.rs` tests pass `tcp_timeout` as third arg
- [ ] `monitors_target_check` constraint covers both kinds explicitly — no "catch-all" that would silently accept future kinds without url or tcp fields
- [ ] `url` is bound as `None` in the TCP INSERT (not omitted) and `tcp_host`/`tcp_port` are bound as `None` in the uptime INSERT
- [ ] `drop(stream)` is present in the `Ok(Ok(stream))` arm of `check_tcp`
- [ ] Unknown-kind arm in the check loop is a separate `_` arm with `tracing::error!` (not inlined into the tcp arm)
- [ ] `run_checker` signature change propagated to `main.rs` spawn call
- [ ] All five SELECT column lists in `monitor/mod.rs` include `tcp_host, tcp_port`
