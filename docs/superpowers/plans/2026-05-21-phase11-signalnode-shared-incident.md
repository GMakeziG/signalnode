# Phase 11: Extract Incident Evaluation to `signalnode-shared` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the duplicated incident open/close state machine from `signalnode-core/src/checker.rs` and `signalnode-api/src/check_result/mod.rs` into a new `signalnode-shared` crate with a single `evaluate_incident` function.

**Architecture:** Create a new `signalnode-shared` workspace member exposing `pub async fn evaluate_incident(tx, monitor_id, workspace_id, failure_threshold, recovery_threshold) -> Result<Option<Uuid>, sqlx::Error>`. Both `signalnode-core` and `signalnode-api` depend on it and replace their ~120 lines of duplicated incident SQL with a single call. Each caller retains its own error handling strategy (core uses `continue`; API returns early with `Internal`).

**Tech Stack:** Rust, sqlx 0.8 (PgPool / Transaction), uuid 1, Cargo workspace.

---

## File Map

| Action   | Path                                          | Responsibility                                 |
|----------|-----------------------------------------------|------------------------------------------------|
| Create   | `signalnode-shared/Cargo.toml`                | New crate manifest                             |
| Create   | `signalnode-shared/src/lib.rs`                | `pub mod incident;`                            |
| Create   | `signalnode-shared/src/incident.rs`           | `evaluate_incident` + integration tests        |
| Modify   | `Cargo.toml`                                  | Add `"signalnode-shared"` to workspace members |
| Modify   | `signalnode-core/Cargo.toml`                  | Add `signalnode-shared` path dependency        |
| Modify   | `signalnode-api/Cargo.toml`                   | Add `signalnode-shared` path dependency        |
| Modify   | `signalnode-core/src/checker.rs`              | Replace incident block with `evaluate_incident` call |
| Modify   | `signalnode-api/src/check_result/mod.rs`      | Replace incident block with `evaluate_incident` call |

---

## Task 1: Create `signalnode-shared` crate scaffold

**Files:**
- Create: `signalnode-shared/Cargo.toml`
- Create: `signalnode-shared/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create `signalnode-shared/Cargo.toml`**

```toml
[package]
name = "signalnode-shared"
version = "0.1.0"
edition = "2021"

[dependencies]
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "macros"] }
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Create `signalnode-shared/src/lib.rs`**

```rust
pub mod incident;
```

- [ ] **Step 3: Create `signalnode-shared/src/incident.rs` with an empty stub**

```rust
use uuid::Uuid;

pub async fn evaluate_incident(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    monitor_id: Uuid,
    workspace_id: Uuid,
    failure_threshold: i32,
    recovery_threshold: i32,
) -> Result<Option<Uuid>, sqlx::Error> {
    let _ = (tx, monitor_id, workspace_id, failure_threshold, recovery_threshold);
    unimplemented!()
}
```

- [ ] **Step 4: Add `signalnode-shared` to the workspace**

Edit `Cargo.toml` at the workspace root (currently `members = ["signalnode-api", "signalnode-core"]`):

```toml
[workspace]
members = ["signalnode-api", "signalnode-core", "signalnode-shared"]
resolver = "2"
```

- [ ] **Step 5: Verify the crate compiles**

Run:
```bash
cargo build -p signalnode-shared
```
Expected: compiles (the `unimplemented!()` body is fine at build time).

- [ ] **Step 6: Commit**

```bash
git add signalnode-shared/Cargo.toml signalnode-shared/src/lib.rs signalnode-shared/src/incident.rs Cargo.toml
git commit -m "feat(shared): scaffold signalnode-shared crate (Phase 11)"
```

---

## Task 2: Write failing tests for `evaluate_incident`

**Files:**
- Modify: `signalnode-shared/src/incident.rs` (add `#[cfg(test)]` module)

- [ ] **Step 1: Add the test module to `signalnode-shared/src/incident.rs`**

Append to the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn setup(pool: &PgPool, failure_threshold: i32, recovery_threshold: i32) -> (Uuid, Uuid) {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) \
             VALUES ('shared-test@example.com', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let wid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (name, slug, owner_id) \
             VALUES ('W', 'shared-test', $1) RETURNING id",
        )
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap();

        let mid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO monitors (workspace_id, name, url, interval_secs, \
             failure_threshold, recovery_threshold) \
             VALUES ($1, 'M', 'http://example.com', 60, $2, $3) RETURNING id",
        )
        .bind(wid)
        .bind(failure_threshold)
        .bind(recovery_threshold)
        .fetch_one(pool)
        .await
        .unwrap();

        (wid, mid)
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn below_threshold_no_incident_opened(pool: PgPool) {
        let (wid, mid) = setup(&pool, 2, 1).await;
        // Only 1 down — threshold is 2, so no incident should open
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'down')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 2, 1).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none(), "should return None when threshold not met");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM incidents WHERE monitor_id = $1")
                .bind(mid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn threshold_crossed_opens_incident_with_fanout(pool: PgPool) {
        let (wid, mid) = setup(&pool, 2, 1).await;
        // Add a notification channel so fanout is exercised
        sqlx::query(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'webhook', 'https://hooks.example.com/p11')",
        )
        .bind(wid)
        .execute(&pool)
        .await
        .unwrap();
        // 2 consecutive down results (older one first)
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'down', NOW() - INTERVAL '10 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'down')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 2, 1).await.unwrap();
        tx.commit().await.unwrap();

        let incident_id = result.expect("should return Some(incident_id) when threshold crossed");
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1, "one open incident");
        let pn: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pending_notifications WHERE incident_id = $1")
                .bind(incident_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pn, 1, "one pending_notification per channel");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_second_incident_when_already_open(pool: PgPool) {
        let (wid, mid) = setup(&pool, 1, 2).await;
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'down')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 1, 2).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none(), "should return None when incident already open");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "still exactly one open incident");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn recovery_threshold_crossed_closes_incident(pool: PgPool) {
        let (wid, mid) = setup(&pool, 1, 2).await;
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        // 2 consecutive up results
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'up', NOW() - INTERVAL '10 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'up')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 1, 2).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none(), "close path returns None");
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        let closed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NOT NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 0);
        assert_eq!(closed, 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn below_recovery_threshold_keeps_incident_open(pool: PgPool) {
        let (wid, mid) = setup(&pool, 1, 2).await;
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        // Only 1 up — recovery_threshold is 2
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'up')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 1, 2).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none());
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1, "incident should remain open");
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail (stub panics)**

Run:
```bash
cargo test -p signalnode-shared 2>&1 | tail -20
```
Expected: tests fail/panic with `not implemented` from the `unimplemented!()` stub.

---

## Task 3: Implement `evaluate_incident`

**Files:**
- Modify: `signalnode-shared/src/incident.rs` (replace the stub body)

- [ ] **Step 1: Replace the `unimplemented!()` stub with the real implementation**

Replace the entire body of `evaluate_incident` (the stub lines only, keep the test module untouched):

```rust
use uuid::Uuid;

pub async fn evaluate_incident(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    monitor_id: Uuid,
    workspace_id: Uuid,
    failure_threshold: i32,
    recovery_threshold: i32,
) -> Result<Option<Uuid>, sqlx::Error> {
    let open_incident = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1",
    )
    .bind(monitor_id)
    .fetch_optional(&mut *tx)
    .await?;

    if open_incident.is_none() {
        let recent = sqlx::query_scalar::<_, String>(
            "SELECT status FROM check_results \
             WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
        )
        .bind(monitor_id)
        .bind(failure_threshold)
        .fetch_all(&mut *tx)
        .await?;

        if recent.len() == failure_threshold as usize && recent.iter().all(|s| s == "down") {
            let incident_id = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO incidents (monitor_id) VALUES ($1) RETURNING id",
            )
            .bind(monitor_id)
            .fetch_one(&mut *tx)
            .await?;

            let channels = sqlx::query_as::<_, (String, String)>(
                "SELECT kind, target FROM notification_channels WHERE workspace_id = $1",
            )
            .bind(workspace_id)
            .fetch_all(&mut *tx)
            .await?;

            for (kind, target) in &channels {
                sqlx::query(
                    "INSERT INTO pending_notifications (incident_id, channel_kind, target) \
                     VALUES ($1, $2, $3)",
                )
                .bind(incident_id)
                .bind(kind)
                .bind(target)
                .execute(&mut *tx)
                .await?;
            }

            return Ok(Some(incident_id));
        }
    } else {
        let recent = sqlx::query_scalar::<_, String>(
            "SELECT status FROM check_results \
             WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
        )
        .bind(monitor_id)
        .bind(recovery_threshold)
        .fetch_all(&mut *tx)
        .await?;

        if recent.len() == recovery_threshold as usize && recent.iter().all(|s| s == "up") {
            sqlx::query(
                "UPDATE incidents SET closed_at = NOW() \
                 WHERE monitor_id = $1 AND closed_at IS NULL",
            )
            .bind(monitor_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    Ok(None)
}
```

- [ ] **Step 2: Run `signalnode-shared` tests to confirm green**

Run:
```bash
cargo test -p signalnode-shared 2>&1 | tail -20
```
Expected: all 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add signalnode-shared/src/incident.rs
git commit -m "feat(shared): implement evaluate_incident with integration tests (Phase 11)"
```

---

## Task 4: Wire `signalnode-core` to use `evaluate_incident`

**Files:**
- Modify: `signalnode-core/Cargo.toml` (add dep)
- Modify: `signalnode-core/src/checker.rs` (replace incident block)

- [ ] **Step 1: Add `signalnode-shared` to `signalnode-core/Cargo.toml`**

Add to `[dependencies]`:

```toml
signalnode-shared = { path = "../signalnode-shared" }
```

- [ ] **Step 2: Replace the incident evaluation block in `checker.rs`**

In `signalnode-core/src/checker.rs`, find the block that starts after the `INSERT INTO check_results` execute (around line 130) and runs to the closing `}` just before `if let Err(e) = tx.commit()` (around line 253). This is the `let open_incident = ...` fetch through the end of the `if open_incident.is_none() { ... } else { ... }` block.

Replace **lines 130–253** with:

```rust
        if let Err(e) = signalnode_shared::incident::evaluate_incident(
            &mut tx,
            outcome.monitor.id,
            outcome.monitor.workspace_id,
            outcome.monitor.failure_threshold,
            outcome.monitor.recovery_threshold,
        )
        .await
        {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: incident evaluation failed");
        }
```

Also remove the comment on line 104:
```
    // Bounded duplication: incident evaluation mirrors signalnode-api/src/check_result/mod.rs
    // Extract to a shared crate in Phase 4.
```

- [ ] **Step 3: Verify `signalnode-core` compiles**

Run:
```bash
cargo build -p signalnode-core
```
Expected: compiles with no errors.

- [ ] **Step 4: Run `signalnode-core` tests**

Run:
```bash
cargo test -p signalnode-core 2>&1 | tail -20
```
Expected: all existing tests pass (33 total).

- [ ] **Step 5: Commit**

```bash
git add signalnode-core/Cargo.toml signalnode-core/src/checker.rs
git commit -m "refactor(core): use signalnode_shared::evaluate_incident in checker (Phase 11)"
```

---

## Task 5: Wire `signalnode-api` to use `evaluate_incident`

**Files:**
- Modify: `signalnode-api/Cargo.toml` (add dep)
- Modify: `signalnode-api/src/check_result/mod.rs` (replace incident block)

- [ ] **Step 1: Add `signalnode-shared` to `signalnode-api/Cargo.toml`**

Add to `[dependencies]`:

```toml
signalnode-shared = { path = "../signalnode-shared" }
```

- [ ] **Step 2: Replace the incident evaluation block in `check_result/mod.rs`**

In `signalnode-api/src/check_result/mod.rs`, find the block that starts with:
```rust
    let (monitor_status, failure_threshold, recovery_threshold) =
```
(around line 113) and ends with the closing `}` of the `if monitor_status == "active" { ... }` block (around line 238).

Replace **lines 113–238** with:

```rust
    let (monitor_status, failure_threshold, recovery_threshold) =
        match sqlx::query_as::<_, (String, i32, i32)>(
            "SELECT status, failure_threshold, recovery_threshold FROM monitors WHERE id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::error!(error = ?e, "failed to fetch monitor for incident evaluation");
                return CheckResultError::Internal.into_response();
            }
        };

    if monitor_status == "active" {
        opened_incident_id = match signalnode_shared::incident::evaluate_incident(
            &mut tx,
            monitor_id,
            workspace_id,
            failure_threshold,
            recovery_threshold,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = ?e, "incident evaluation failed");
                return CheckResultError::Internal.into_response();
            }
        };
    }
```

- [ ] **Step 3: Verify `signalnode-api` compiles**

Run:
```bash
cargo build -p signalnode-api
```
Expected: compiles with no errors.

- [ ] **Step 4: Run `signalnode-api` tests**

Run:
```bash
cargo test -p signalnode-api 2>&1 | tail -20
```
Expected: all existing tests pass (145 total).

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/Cargo.toml signalnode-api/src/check_result/mod.rs
git commit -m "refactor(api): use signalnode_shared::evaluate_incident in check_result (Phase 11)"
```

---

## Task 6: Full workspace verification

- [ ] **Step 1: Run full test suite**

Run:
```bash
cargo test --workspace 2>&1 | tail -30
```
Expected: all tests pass. Count should be ≥ 183 (178 existing + 5 new in signalnode-shared).

- [ ] **Step 2: Confirm no dead code warnings related to removed duplication**

Run:
```bash
cargo build --workspace 2>&1 | grep -i "warning"
```
Expected: no new warnings.
