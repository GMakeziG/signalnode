# Phase 10: Authorization Helper Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate four identical copies of `check_membership` and two identical copies of `check_owner` into a single `signalnode-api/src/authz.rs` module.

**Architecture:** New `authz.rs` exports `AuthzError` (implementing `IntoResponse` directly) and two public async helpers. Each of the four API modules drops its local private copies, adds `use crate::authz;`, and replaces bare calls with `authz::check_membership` / `authz::check_owner`. Handler early-return pattern is unchanged.

**Tech Stack:** Rust, Axum, sqlx (PgPool), `#[sqlx::test]` integration tests against real Postgres.

**Spec:** `docs/superpowers/specs/2026-05-21-phase10-authz-consolidation-design.md`

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `signalnode-api/src/authz.rs` | `AuthzError`, `check_membership`, `check_owner`, unit tests |
| Modify | `signalnode-api/src/lib.rs` | register `pub mod authz;` |
| Modify | `signalnode-api/src/monitor/mod.rs` | delete local helpers, use `authz::` |
| Modify | `signalnode-api/src/incident/mod.rs` | delete local helper, use `authz::` |
| Modify | `signalnode-api/src/notification_channel/mod.rs` | delete local helpers, use `authz::` |
| Modify | `signalnode-api/src/check_result/mod.rs` | delete local helper, use `authz::` |

---

## Task 1: Create `authz.rs` with `AuthzError` and `check_membership` (TDD)

**Files:**
- Create: `signalnode-api/src/authz.rs`
- Modify: `signalnode-api/src/lib.rs`

- [ ] **Step 1: Write the failing tests for `check_membership`**

Create `signalnode-api/src/authz.rs` with the test block only (no implementation yet):

```rust
#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{check_membership, AuthzError};
    use crate::test_helpers::{create_test_user, create_test_workspace};

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_ok_for_member(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // create_test_workspace inserts an owner membership
        assert!(check_membership(&pool, wid, uid).await.is_ok());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_forbidden_for_non_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let outsider = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        let result = check_membership(&pool, wid, outsider).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_not_found_for_missing_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4(); // does not exist
        let result = check_membership(&pool, wid, uid).await;
        assert!(matches!(result, Err(AuthzError::NotFound)));
    }
}
```

- [ ] **Step 2: Register the module so the tests can compile**

In `signalnode-api/src/lib.rs`, add after `pub mod auth;`:

```rust
pub mod authz;
```

- [ ] **Step 3: Run tests to confirm they fail with "unresolved items"**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api authz 2>&1 | tail -20
```

Expected: compile error — `check_membership`, `AuthzError` not found.

- [ ] **Step 4: Implement `AuthzError` and `check_membership`**

Replace the contents of `signalnode-api/src/authz.rs` with:

```rust
use axum::{response::{IntoResponse, Response}, http::StatusCode, Json};
use sqlx::PgPool;
use std::borrow::Cow;
use uuid::Uuid;

use crate::ErrorBody;

pub enum AuthzError {
    Forbidden,
    NotFound,
    Internal,
}

impl IntoResponse for AuthzError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AuthzError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            AuthzError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            AuthzError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}

pub async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzError> {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => {
            match sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
            )
            .bind(workspace_id)
            .fetch_one(pool)
            .await
            {
                Ok(true) => Err(AuthzError::Forbidden),
                Ok(false) => Err(AuthzError::NotFound),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(AuthzError::Internal)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(AuthzError::Internal)
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{check_membership, AuthzError};
    use crate::test_helpers::{create_test_user, create_test_workspace};

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_ok_for_member(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        assert!(check_membership(&pool, wid, uid).await.is_ok());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_forbidden_for_non_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let outsider = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        let result = check_membership(&pool, wid, outsider).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_not_found_for_missing_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4();
        let result = check_membership(&pool, wid, uid).await;
        assert!(matches!(result, Err(AuthzError::NotFound)));
    }
}
```

- [ ] **Step 5: Run the three new tests**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api authz 2>&1 | tail -20
```

Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
cd /home/ninjatronics/src/signalnode && git add signalnode-api/src/authz.rs signalnode-api/src/lib.rs && git commit -m "feat(api): add authz module with AuthzError and check_membership (Phase 10)"
```

---

## Task 2: Add `check_owner` to `authz.rs` (TDD)

**Files:**
- Modify: `signalnode-api/src/authz.rs`

- [ ] **Step 1: Write failing tests for `check_owner`**

Append to the `tests` module inside `signalnode-api/src/authz.rs` (inside the existing `#[cfg(test)] mod tests { ... }` block, before the closing `}`):

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn owner_ok_for_owner(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // create_test_workspace inserts role = 'owner'
        assert!(check_owner(&pool, wid, uid).await.is_ok());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_forbidden_for_plain_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let member = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(wid)
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
        let result = check_owner(&pool, wid, member).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_forbidden_for_non_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let outsider = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        let result = check_owner(&pool, wid, outsider).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_not_found_for_missing_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4();
        let result = check_owner(&pool, wid, uid).await;
        assert!(matches!(result, Err(AuthzError::NotFound)));
    }
```

Also add `check_owner` to the `use super::` line:

```rust
    use super::{check_membership, check_owner, AuthzError};
```

- [ ] **Step 2: Run tests to confirm the four new ones fail**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api authz 2>&1 | tail -20
```

Expected: compile error — `check_owner` not found.

- [ ] **Step 3: Implement `check_owner`**

Add the following function to `signalnode-api/src/authz.rs` after `check_membership` (before `#[cfg(test)]`):

```rust
pub async fn check_owner(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzError> {
    match sqlx::query_scalar::<_, String>(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(role)) if role == "owner" => Ok(()),
        Ok(Some(_)) => Err(AuthzError::Forbidden),
        Ok(None) => match sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
        )
        .bind(workspace_id)
        .fetch_one(pool)
        .await
        {
            Ok(true) => Err(AuthzError::Forbidden),
            Ok(false) => Err(AuthzError::NotFound),
            Err(e) => {
                tracing::error!(error = ?e, "failed to check workspace existence");
                Err(AuthzError::Internal)
            }
        },
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace owner");
            Err(AuthzError::Internal)
        }
    }
}
```

- [ ] **Step 4: Run all 7 authz tests**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api authz 2>&1 | tail -20
```

Expected: 7 tests pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ninjatronics/src/signalnode && git add signalnode-api/src/authz.rs && git commit -m "feat(api): add check_owner to authz module (Phase 10)"
```

---

## Task 3: Migrate `monitor` module

**Files:**
- Modify: `signalnode-api/src/monitor/mod.rs`

The monitor module has both `check_membership` (≈lines 69–105) and `check_owner` (≈lines 106–140), called from 5 handlers.

- [ ] **Step 1: Delete the two local helpers**

In `signalnode-api/src/monitor/mod.rs`, delete the entire bodies of `async fn check_membership` and `async fn check_owner` (everything from `async fn check_membership(` through the closing `}` of `check_owner`). These are the two private functions before `async fn create_monitor`.

- [ ] **Step 2: Add the authz import**

In `signalnode-api/src/monitor/mod.rs`, add to the `use crate::` block:

```rust
use crate::authz;
```

- [ ] **Step 3: Update all call sites in the module**

Replace every occurrence of:
```rust
check_membership(&state.pool,
```
with:
```rust
authz::check_membership(&state.pool,
```

Replace every occurrence of:
```rust
check_owner(&state.pool,
```
with:
```rust
authz::check_owner(&state.pool,
```

There are 4 `check_membership` call sites and 1 `check_owner` call site in this file.

- [ ] **Step 4: Run all API tests**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api 2>&1 | tail -20
```

Expected: all tests pass (count unchanged from pre-migration).

- [ ] **Step 5: Commit**

```bash
cd /home/ninjatronics/src/signalnode && git add signalnode-api/src/monitor/mod.rs && git commit -m "refactor(api): use authz::check_membership/check_owner in monitor (Phase 10)"
```

---

## Task 4: Migrate `incident` module

**Files:**
- Modify: `signalnode-api/src/incident/mod.rs`

The incident module has only `check_membership` (≈lines 31–60), called from 1 handler.

- [ ] **Step 1: Delete the local `check_membership` helper**

In `signalnode-api/src/incident/mod.rs`, delete the entire `async fn check_membership` function body.

- [ ] **Step 2: Add the authz import**

```rust
use crate::authz;
```

- [ ] **Step 3: Update the call site**

Replace:
```rust
check_membership(&state.pool,
```
with:
```rust
authz::check_membership(&state.pool,
```

- [ ] **Step 4: Run all API tests**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ninjatronics/src/signalnode && git add signalnode-api/src/incident/mod.rs && git commit -m "refactor(api): use authz::check_membership in incident (Phase 10)"
```

---

## Task 5: Migrate `notification_channel` module

**Files:**
- Modify: `signalnode-api/src/notification_channel/mod.rs`

The notification_channel module has both `check_membership` (≈lines 45–80) and `check_owner` (≈lines 82–120), with 2 call sites each.

- [ ] **Step 1: Delete both local helpers**

In `signalnode-api/src/notification_channel/mod.rs`, delete both `async fn check_membership` and `async fn check_owner`.

- [ ] **Step 2: Add the authz import**

```rust
use crate::authz;
```

- [ ] **Step 3: Update all call sites**

Replace every:
```rust
check_membership(&state.pool,
```
with:
```rust
authz::check_membership(&state.pool,
```

Replace every:
```rust
check_owner(&state.pool,
```
with:
```rust
authz::check_owner(&state.pool,
```

There are 2 `check_membership` and 2 `check_owner` call sites in this file.

- [ ] **Step 4: Run all API tests**

```bash
cd /home/ninjatronics/src/signalnode && cargo test -p signalnode-api 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ninjatronics/src/signalnode && git add signalnode-api/src/notification_channel/mod.rs && git commit -m "refactor(api): use authz::check_membership/check_owner in notification_channel (Phase 10)"
```

---

## Task 6: Migrate `check_result` module and final verification

**Files:**
- Modify: `signalnode-api/src/check_result/mod.rs`

The check_result module has only `check_membership` (≈lines 42–65), called from 2 handlers.

- [ ] **Step 1: Delete the local `check_membership` helper**

In `signalnode-api/src/check_result/mod.rs`, delete the entire `async fn check_membership` function body.

- [ ] **Step 2: Add the authz import**

```rust
use crate::authz;
```

- [ ] **Step 3: Update both call sites**

Replace every:
```rust
check_membership(&state.pool,
```
with:
```rust
authz::check_membership(&state.pool,
```

There are 2 call sites in this file.

- [ ] **Step 4: Run the full test suite (both crates)**

```bash
cd /home/ninjatronics/src/signalnode && cargo test 2>&1 | tail -30
```

Expected: all 178 tests pass (171 pre-Phase-10 + 7 new authz tests). Zero failures.

- [ ] **Step 5: Commit**

```bash
cd /home/ninjatronics/src/signalnode && git add signalnode-api/src/check_result/mod.rs && git commit -m "refactor(api): use authz::check_membership in check_result (Phase 10)"
```
