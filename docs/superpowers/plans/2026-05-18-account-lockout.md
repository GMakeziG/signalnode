# Account Lockout (Phase 6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lock `/auth/login` for 15 minutes after 10 consecutive failed attempts, keyed per-account (user_id), with auto-unlock and count reset on successful login.

**Architecture:** One new Postgres migration adds `login_attempts(user_id PK FK, failed_count, locked_until, last_failed_at)`. The `login` handler gains three new DB operations: a lockout check before bcrypt, an atomic upsert on wrong password, and a DELETE on success. No new files — all changes land in `signalnode-api/src/auth/mod.rs` and `migrations/`.

**Tech Stack:** Rust, Axum, sqlx (Postgres), existing `AppState { pool, jwt_secret }`.

---

## File Map

| File | Change |
|---|---|
| `migrations/20260518000013_login_attempts.sql` | **Create** — new table |
| `signalnode-api/src/auth/mod.rs` | **Modify** — `login` handler + 6 new tests |

---

## Task 1: Migration — create `login_attempts` table

**Files:**
- Create: `migrations/20260518000013_login_attempts.sql`

- [ ] **Step 1: Write the migration**

```sql
-- migrations/20260518000013_login_attempts.sql
CREATE TABLE login_attempts (
    user_id        UUID        PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    failed_count   INT         NOT NULL DEFAULT 0,
    locked_until   TIMESTAMPTZ,
    last_failed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

`locked_until` is nullable — `NULL` means not locked. `locked_until > NOW()` means actively locked. Rows are deleted on successful login; there is no explicit NULL reset. No extra indexes: all lookups are point queries by `user_id` (PK).

- [ ] **Step 2: Verify migration applies cleanly**

Run:
```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml -- --list 2>&1 | head -5
```

Expected: compiles without error. The `sqlx::test` harness runs all migrations against a fresh DB per test — if the migration file has a syntax error, tests will fail with a migration error rather than an assertion error.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260518000013_login_attempts.sql
git commit -m "feat(db): add login_attempts table for account lockout"
```

---

## Task 2: Write six failing tests (TDD red)

**Files:**
- Modify: `signalnode-api/src/auth/mod.rs` — append to the `#[cfg(test)] mod tests` block

Add the following six tests inside the existing `mod tests { ... }` block in `signalnode-api/src/auth/mod.rs`. All use the existing `post_json` helper and `TEST_JWT_SECRET` constant already defined in that block. `PgPool` is `Clone` — pass `pool.clone()` to every `post_json` call and keep the original for direct DB assertions.

- [ ] **Step 1: Append the six tests**

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn login_locked_account_returns_401(pool: PgPool) {
        post_json(
            pool.clone(),
            "/auth/register",
            json!({"email": "lock@example.com", "password": "password123"}),
        )
        .await;

        // 10 consecutive failures — 10th sets locked_until
        for _ in 0..10 {
            post_json(
                pool.clone(),
                "/auth/login",
                json!({"email": "lock@example.com", "password": "wrongpass1"}),
            )
            .await;
        }

        // 11th attempt with CORRECT password still returns 401 (lockout active)
        let res = post_json(
            pool.clone(),
            "/auth/login",
            json!({"email": "lock@example.com", "password": "password123"}),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_lockout_not_triggered_before_threshold(pool: PgPool) {
        post_json(
            pool.clone(),
            "/auth/register",
            json!({"email": "almost@example.com", "password": "password123"}),
        )
        .await;

        // 9 failures — must NOT lock the account
        for _ in 0..9 {
            post_json(
                pool.clone(),
                "/auth/login",
                json!({"email": "almost@example.com", "password": "wrongpass1"}),
            )
            .await;
        }

        // 10th attempt with correct password succeeds (not locked)
        let res = post_json(
            pool.clone(),
            "/auth/login",
            json!({"email": "almost@example.com", "password": "password123"}),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_successful_resets_failure_count(pool: PgPool) {
        post_json(
            pool.clone(),
            "/auth/register",
            json!({"email": "reset@example.com", "password": "password123"}),
        )
        .await;

        // Fail 9 times
        for _ in 0..9 {
            post_json(
                pool.clone(),
                "/auth/login",
                json!({"email": "reset@example.com", "password": "wrongpass1"}),
            )
            .await;
        }

        // Succeed once — DELETE removes the row
        post_json(
            pool.clone(),
            "/auth/login",
            json!({"email": "reset@example.com", "password": "password123"}),
        )
        .await;

        // Fail once more — creates a fresh row with failed_count = 1
        post_json(
            pool.clone(),
            "/auth/login",
            json!({"email": "reset@example.com", "password": "wrongpass1"}),
        )
        .await;

        let count: Option<i32> = sqlx::query_scalar(
            "SELECT failed_count FROM login_attempts \
             WHERE user_id = (SELECT id FROM users WHERE email = $1)",
        )
        .bind("reset@example.com")
        .fetch_optional(&pool)
        .await
        .unwrap();

        assert_eq!(count, Some(1));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_success_after_lockout_expires(pool: PgPool) {
        post_json(
            pool.clone(),
            "/auth/register",
            json!({"email": "expired@example.com", "password": "password123"}),
        )
        .await;

        // Trigger lockout
        for _ in 0..10 {
            post_json(
                pool.clone(),
                "/auth/login",
                json!({"email": "expired@example.com", "password": "wrongpass1"}),
            )
            .await;
        }

        // Manually expire the lock — avoids sleeping in tests
        sqlx::query(
            "UPDATE login_attempts \
             SET locked_until = NOW() - INTERVAL '1 second' \
             WHERE user_id = (SELECT id FROM users WHERE email = $1)",
        )
        .bind("expired@example.com")
        .execute(&pool)
        .await
        .unwrap();

        // Login with correct password now succeeds
        let res = post_json(
            pool.clone(),
            "/auth/login",
            json!({"email": "expired@example.com", "password": "password123"}),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_wrong_password_increments_count(pool: PgPool) {
        post_json(
            pool.clone(),
            "/auth/register",
            json!({"email": "count@example.com", "password": "password123"}),
        )
        .await;

        for _ in 0..3 {
            post_json(
                pool.clone(),
                "/auth/login",
                json!({"email": "count@example.com", "password": "wrongpass1"}),
            )
            .await;
        }

        let count: Option<i32> = sqlx::query_scalar(
            "SELECT failed_count FROM login_attempts \
             WHERE user_id = (SELECT id FROM users WHERE email = $1)",
        )
        .bind("count@example.com")
        .fetch_optional(&pool)
        .await
        .unwrap();

        assert_eq!(count, Some(3));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_correct_password_deletes_attempt_row(pool: PgPool) {
        post_json(
            pool.clone(),
            "/auth/register",
            json!({"email": "cleanup@example.com", "password": "password123"}),
        )
        .await;

        // Fail 5 times to create a row in login_attempts
        for _ in 0..5 {
            post_json(
                pool.clone(),
                "/auth/login",
                json!({"email": "cleanup@example.com", "password": "wrongpass1"}),
            )
            .await;
        }

        // Succeed — DELETE should remove the row
        post_json(
            pool.clone(),
            "/auth/login",
            json!({"email": "cleanup@example.com", "password": "password123"}),
        )
        .await;

        let row_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM login_attempts \
             WHERE user_id = (SELECT id FROM users WHERE email = $1)",
        )
        .bind("cleanup@example.com")
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row_count, 0);
    }
```

- [ ] **Step 2: Run the new tests to confirm they fail (red)**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  "auth::tests::login_locked" \
  "auth::tests::login_lockout" \
  "auth::tests::login_successful_resets" \
  "auth::tests::login_success_after_lockout" \
  "auth::tests::login_wrong_password_increments" \
  "auth::tests::login_correct_password_deletes" \
  -- --nocapture 2>&1 | tail -30
```

Expected: tests that require lockout logic (`login_locked_account_returns_401`, `login_lockout_not_triggered_before_threshold`, `login_success_after_lockout_expires`) fail with assertion errors (wrong status code). Tests that only assert DB state (`login_wrong_password_increments_count`, `login_correct_password_deletes_attempt_row`) fail with `None != Some(...)` because the handler doesn't write to `login_attempts` yet.

- [ ] **Step 3: Commit the failing tests**

```bash
git add signalnode-api/src/auth/mod.rs
git commit -m "test(auth): add failing account lockout tests (red)"
```

---

## Task 3: Implement lockout in the login handler (TDD green)

**Files:**
- Modify: `signalnode-api/src/auth/mod.rs` — replace the `login` function body

Replace the entire `login` function (lines roughly 122–197 in the current file) with the version below. The existing `register` and `refresh` functions are untouched.

- [ ] **Step 1: Replace the `login` function**

```rust
async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> impl IntoResponse {
    let row =
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, password_hash FROM users WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.pool)
            .await;

    let (user_id, password_hash) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            // Dummy verify to equalise timing with the found-user path
            let _ = tokio::task::spawn_blocking(|| verify_password("x", dummy_hash())).await;
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "database error during login");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Lockout check runs before bcrypt to avoid CPU cost on locked accounts.
    // Locked accounts return a generic 401 — same as wrong credentials — to
    // avoid revealing whether the account exists or is locked. See Phase 6 spec.
    let is_locked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(\
            SELECT 1 FROM login_attempts \
            WHERE user_id = $1 AND locked_until IS NOT NULL AND locked_until > NOW()\
        )",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await;

    match is_locked {
        Ok(true) => return StatusCode::UNAUTHORIZED.into_response(),
        Ok(false) => {}
        Err(e) => {
            tracing::error!(error = ?e, "database error checking account lockout");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let password = body.password.clone();
    let ok = match tokio::task::spawn_blocking(move || verify_password(&password, &password_hash))
        .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::error!(error = ?e, "password verification error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "spawn_blocking panicked during verify");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !ok {
        // Atomic upsert: insert first failure or increment existing count.
        // locked_until is set on the 10th failure (failed_count + 1 >= 10
        // where failed_count is the pre-update stored value).
        // The 10th attempt returns a normal wrong-password 401 after setting
        // locked_until; the 11th attempt hits the lockout check above.
        let upsert = sqlx::query(
            "INSERT INTO login_attempts (user_id, failed_count, last_failed_at) \
             VALUES ($1, 1, NOW()) \
             ON CONFLICT (user_id) DO UPDATE SET \
                 failed_count   = login_attempts.failed_count + 1, \
                 last_failed_at = NOW(), \
                 locked_until   = CASE \
                     WHEN login_attempts.failed_count + 1 >= 10 \
                     THEN NOW() + INTERVAL '15 minutes' \
                     ELSE login_attempts.locked_until \
                 END",
        )
        .bind(user_id)
        .execute(&state.pool)
        .await;

        if let Err(e) = upsert {
            tracing::error!(error = ?e, "database error recording failed login attempt");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Successful login: remove all failure state for this account.
    if let Err(e) = sqlx::query("DELETE FROM login_attempts WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await
    {
        tracing::error!(error = ?e, "database error clearing login attempts on success");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let uid = user_id.to_string();
    let access_token = match encode_access_token(&uid, &state.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = ?e, "access token encoding failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let (refresh_token, refresh_jti, refresh_expires_at) =
        match encode_refresh_token(&uid, &state.jwt_secret) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = ?e, "refresh token encoding failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

    let result = sqlx::query(
        "INSERT INTO refresh_tokens (jti, user_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(refresh_jti)
    .bind(user_id)
    .bind(refresh_expires_at)
    .execute(&state.pool)
    .await;

    if let Err(e) = result {
        tracing::error!(error = ?e, "failed to persist refresh token");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(AuthResponse {
        access_token,
        refresh_token,
    })
    .into_response()
}
```

- [ ] **Step 2: Run the six new tests (green)**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  "auth::tests::login_locked" \
  "auth::tests::login_lockout" \
  "auth::tests::login_successful_resets" \
  "auth::tests::login_success_after_lockout" \
  "auth::tests::login_wrong_password_increments" \
  "auth::tests::login_correct_password_deletes" \
  -- --nocapture 2>&1 | tail -20
```

Expected: all 6 PASS.

- [ ] **Step 3: Run the full signalnode-api test suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -20
```

Expected: all tests pass (previously 125 + 6 new = 131). No regressions.

- [ ] **Step 4: Run signalnode-core tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml 2>&1 | tail -10
```

Expected: all 29 pass (core is unchanged; this confirms the migration didn't break anything shared).

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/auth/mod.rs
git commit -m "feat(auth): account lockout after 10 consecutive failed logins"
```

---

## Task 4: Update handoff

**Files:**
- Modify: memory file at `~/.claude/projects/-home-ninjatronics-src-signalnode/memory/handoff_session.md`

- [ ] **Step 1: Update the handoff memory**

Update the handoff to record Phase 6 as complete:
- Add Phase 6 row to the Completed Phases table
- Update test counts (125 + 6 = 131 signalnode-api, total 160)
- Promote security debt item #2 (`refresh_tokens` cleanup) to Recommended Next
- Remove account lockout from security debt list #1

- [ ] **Step 2: Commit**

```bash
git add ~/.claude/projects/-home-ninjatronics-src-signalnode/memory/handoff_session.md 2>/dev/null || true
# Memory files live outside the repo — no git commit needed for them.
# Commit any docs changes if applicable:
git status
```

---

## Self-Review

**Spec coverage:**
- ✅ Migration: `login_attempts` table — Task 1
- ✅ Lockout check before bcrypt — Task 3 handler, step 1 (`is_locked` query)
- ✅ Atomic upsert on wrong password, threshold 10, 15-minute window — Task 3 handler, step 1 (`upsert` query)
- ✅ DELETE on success (count reset) — Task 3 handler, step 1 (`DELETE` query)
- ✅ Generic 401 for locked accounts — Task 3 handler, step 1 (returns `UNAUTHORIZED`)
- ✅ 10th attempt returns wrong-password 401 (not lockout path), 11th hits lockout — documented in handler comment and `login_locked_account_returns_401` test uses correct password on 11th
- ✅ All 6 tests — Task 2
- ✅ `login_success_after_lockout_expires` uses direct DB update, not sleep — Task 2 step 1
- ✅ No admin endpoint — nothing added
- ✅ Future work documented in spec — not repeated here

**Placeholder scan:** No TBD/TODO. All SQL is complete. All test code is complete.

**Type consistency:** `user_id: Uuid` used throughout. `query_scalar::<_, bool>` for EXISTS check. `query_scalar::<_, Option<i32>>` / `query_scalar::<_, i64>` for DB assertions in tests — both match the Postgres column types (`INT` → `i32`, `COUNT(*)` → `i64`).
