# Refresh Token Rotation + Replay Protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace stateless refresh tokens with DB-backed single-use tokens that are atomically rotated on every `/auth/refresh` call, making stolen tokens useless after first legitimate use.

**Architecture:** A new `refresh_tokens` table stores a UUID `jti` (JWT ID) per issued refresh token. `encode_refresh_token` embeds the jti in the JWT and returns it to the caller for persistence. The `/auth/refresh` handler atomically DELETEs the presented jti row (proving it was never used before) and INSERTs a fresh jti row in the same transaction before issuing a new token pair. A missing or already-deleted jti produces a 401.

**Tech Stack:** sqlx 0.8 (postgres + uuid + chrono), jsonwebtoken 9, chrono 0.4, uuid 1 — all already in `signalnode-api/Cargo.toml`. No new dependencies.

---

## File Map

| Action | Path | Responsibility |
|--------|------|---------------|
| Create | `migrations/20260518000012_refresh_tokens.sql` | `refresh_tokens` table and index |
| Modify | `signalnode-api/src/auth/token.rs` | Add `jti: Option<String>` to `Claims`; change `encode_refresh_token` to return `(String, Uuid, DateTime<Utc>)` |
| Modify | `signalnode-api/src/auth/mod.rs` | `login` persists jti; `refresh` validates + atomically rotates; returns `AuthResponse` (both tokens) |

---

## Task 1: Migration — refresh_tokens table

**Files:**
- Create: `migrations/20260518000012_refresh_tokens.sql`

- [ ] **Step 1: Write the migration**

```sql
-- migrations/20260518000012_refresh_tokens.sql
CREATE TABLE refresh_tokens (
    jti        UUID        PRIMARY KEY,
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX refresh_tokens_user_id_idx ON refresh_tokens (user_id);
```

- [ ] **Step 2: Verify the migration applies cleanly**

`#[sqlx::test(migrations = "../migrations")]` automatically runs all migrations for every test. Run the full suite to confirm the new migration applies without error:

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -20
```

Expected: all existing tests still pass (116 pass, 0 fail). The migration is additive — nothing reads or writes the new table yet.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260518000012_refresh_tokens.sql
git commit -m "feat: add refresh_tokens table for DB-backed token rotation"
```

---

## Task 2: Add jti claim to refresh tokens

`encode_refresh_token` currently returns `Result<String, _>`. After this task it returns `Result<(String, Uuid, DateTime<Utc>), _>` so callers can persist the jti without re-parsing the JWT.

**Files:**
- Modify: `signalnode-api/src/auth/token.rs`

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `mod tests` block in `signalnode-api/src/auth/token.rs`:

```rust
    #[test]
    fn refresh_token_has_jti() {
        let (token, _jti, _exp) = encode_refresh_token("user-123", SECRET).unwrap();
        let claims = decode_refresh_token(&token, SECRET).unwrap();
        assert!(claims.jti.is_some());
        assert!(!claims.jti.as_deref().unwrap_or("").is_empty());
    }

    #[test]
    fn access_token_has_no_jti() {
        let token = encode_access_token("user-123", SECRET).unwrap();
        let claims = decode_access_token(&token, SECRET).unwrap();
        assert!(claims.jti.is_none());
    }

    #[test]
    fn refresh_token_jtis_are_unique() {
        let (_, jti1, _) = encode_refresh_token("user-123", SECRET).unwrap();
        let (_, jti2, _) = encode_refresh_token("user-123", SECRET).unwrap();
        assert_ne!(jti1, jti2);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml auth::token 2>&1 | tail -20
```

Expected: compilation errors — `encode_refresh_token` still returns `String`, not a tuple; `Claims` has no `jti` field.

- [ ] **Step 3: Update `Claims` and `encode_refresh_token` in token.rs**

Replace the entire contents of `signalnode-api/src/auth/token.rs` with:

```rust
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{
    decode, encode, errors::ErrorKind, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ACCESS_TOKEN_MINUTES: i64 = 15;
pub const REFRESH_TOKEN_DAYS: i64 = 7;
const KIND_ACCESS: &str = "access";
const KIND_REFRESH: &str = "refresh";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
    pub kind: String,
    #[serde(default)]
    pub jti: Option<String>,
}

pub fn encode_access_token(
    user_id: &str,
    secret: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = (Utc::now() + Duration::minutes(ACCESS_TOKEN_MINUTES)).timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        exp,
        kind: KIND_ACCESS.to_string(),
        jti: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

/// Returns (jwt_string, jti, expires_at). Caller must persist jti in refresh_tokens.
pub fn encode_refresh_token(
    user_id: &str,
    secret: &str,
) -> Result<(String, Uuid, DateTime<Utc>), jsonwebtoken::errors::Error> {
    let jti = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(REFRESH_TOKEN_DAYS);
    let claims = Claims {
        sub: user_id.to_string(),
        exp: expires_at.timestamp(),
        kind: KIND_REFRESH.to_string(),
        jti: Some(jti.to_string()),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok((token, jti, expires_at))
}

pub fn decode_access_token(
    token: &str,
    secret: &str,
) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    if data.claims.kind != KIND_ACCESS {
        return Err(ErrorKind::InvalidToken.into());
    }
    Ok(data.claims)
}

pub fn decode_refresh_token(
    token: &str,
    secret: &str,
) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    if data.claims.kind != KIND_REFRESH {
        return Err(ErrorKind::InvalidToken.into());
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-at-least-32-chars-long!";

    #[test]
    fn encode_decode_access_roundtrip() {
        let token = encode_access_token("user-id-123", SECRET).unwrap();
        let claims = decode_access_token(&token, SECRET).unwrap();
        assert_eq!(claims.sub, "user-id-123");
        assert_eq!(claims.kind, "access");
    }

    #[test]
    fn encode_decode_refresh_roundtrip() {
        let (token, _, _) = encode_refresh_token("user-id-456", SECRET).unwrap();
        let claims = decode_refresh_token(&token, SECRET).unwrap();
        assert_eq!(claims.sub, "user-id-456");
        assert_eq!(claims.kind, "refresh");
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = encode_access_token("user-id-123", SECRET).unwrap();
        assert!(decode_access_token(&token, "wrong-secret-padding-padding-ppp").is_err());
    }

    #[test]
    fn access_token_rejected_as_refresh() {
        let token = encode_access_token("uid", SECRET).unwrap();
        assert!(decode_refresh_token(&token, SECRET).is_err());
    }

    #[test]
    fn refresh_token_rejected_as_access() {
        let (token, _, _) = encode_refresh_token("uid", SECRET).unwrap();
        assert!(decode_access_token(&token, SECRET).is_err());
    }

    #[test]
    fn refresh_token_has_jti() {
        let (token, _jti, _exp) = encode_refresh_token("user-123", SECRET).unwrap();
        let claims = decode_refresh_token(&token, SECRET).unwrap();
        assert!(claims.jti.is_some());
        assert!(!claims.jti.as_deref().unwrap_or("").is_empty());
    }

    #[test]
    fn access_token_has_no_jti() {
        let token = encode_access_token("user-123", SECRET).unwrap();
        let claims = decode_access_token(&token, SECRET).unwrap();
        assert!(claims.jti.is_none());
    }

    #[test]
    fn refresh_token_jtis_are_unique() {
        let (_, jti1, _) = encode_refresh_token("user-123", SECRET).unwrap();
        let (_, jti2, _) = encode_refresh_token("user-123", SECRET).unwrap();
        assert_ne!(jti1, jti2);
    }
}
```

- [ ] **Step 4: Fix the compile error in mod.rs caused by the signature change**

The `login` handler in `signalnode-api/src/auth/mod.rs` still destructures `encode_refresh_token` as a `String`. Fix that one call site — do not add DB persistence yet (that is Task 3):

In `mod.rs`, find the `login` handler block that calls `encode_refresh_token` and change it from:

```rust
    let refresh_token = match encode_refresh_token(&uid, &state.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = ?e, "refresh token encoding failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
```

to:

```rust
    let (refresh_token, _refresh_jti, _refresh_expires_at) =
        match encode_refresh_token(&uid, &state.jwt_secret) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = ?e, "refresh token encoding failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
```

- [ ] **Step 5: Run all tests and verify they pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -20
```

Expected: all 119 tests pass (116 existing + 3 new token unit tests).

- [ ] **Step 6: Commit**

```bash
git add signalnode-api/src/auth/token.rs
git commit -m "feat(api): add jti claim to refresh tokens; encode_refresh_token returns (token, jti, expires_at)"
```

---

## Task 3: Login persists refresh token jti

**Files:**
- Modify: `signalnode-api/src/auth/mod.rs`

- [ ] **Step 1: Write the failing test**

Add this test inside the `mod tests` block in `signalnode-api/src/auth/mod.rs`:

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn login_persists_refresh_token_in_db(pool: PgPool) {
        let pool2 = pool.clone();
        post_json(
            pool,
            "/auth/register",
            json!({"email": "jti@example.com", "password": "password123"}),
        )
        .await;
        post_json(
            pool2.clone(),
            "/auth/login",
            json!({"email": "jti@example.com", "password": "password123"}),
        )
        .await;
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM refresh_tokens")
            .fetch_one(&pool2)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml login_persists 2>&1 | tail -10
```

Expected: FAIL — count is 0 because login does not yet insert into refresh_tokens.

- [ ] **Step 3: Update the `login` handler to persist the jti**

In `signalnode-api/src/auth/mod.rs`, replace the temporary `_refresh_jti`/`_refresh_expires_at` destructure from Task 2 with the full persistence logic. The full updated `login` handler (replace the entire `async fn login` function):

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
            let _ = tokio::task::spawn_blocking(|| verify_password("x", dummy_hash())).await;
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "database error during login");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

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
        return StatusCode::UNAUTHORIZED.into_response();
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

- [ ] **Step 4: Run all tests and verify they pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -20
```

Expected: 120 tests pass (119 from Task 2 + 1 new).

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/auth/mod.rs
git commit -m "feat(api): persist refresh token jti to DB on login"
```

---

## Task 4: Refresh — validate jti, rotate, return new token pair

This is the core security change. The `/auth/refresh` handler now:
1. Validates the JWT signature and expiry (existing)
2. Extracts the jti from claims (new)
3. Atomically DELETEs the jti row — if 0 rows deleted, the token was already used → 401
4. INSERTs a fresh jti row in the same transaction
5. Returns `AuthResponse` with both a new access token and a new refresh token

**Files:**
- Modify: `signalnode-api/src/auth/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add both tests inside the `mod tests` block in `signalnode-api/src/auth/mod.rs`:

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn refresh_returns_new_token_pair(pool: PgPool) {
        let p2 = pool.clone();
        let p3 = pool.clone();
        post_json(
            pool,
            "/auth/register",
            json!({"email": "rotate@example.com", "password": "password123"}),
        )
        .await;
        let login_res = post_json(
            p2,
            "/auth/login",
            json!({"email": "rotate@example.com", "password": "password123"}),
        )
        .await;
        let body = axum::body::to_bytes(login_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let tokens: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let refresh_token = tokens["refresh_token"].as_str().unwrap().to_string();

        let res = post_json(p3, "/auth/refresh", json!({"refresh_token": refresh_token})).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["access_token"].is_string());
        assert!(json["refresh_token"].is_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn refresh_token_is_single_use(pool: PgPool) {
        let p2 = pool.clone();
        let p3 = pool.clone();
        let p4 = pool.clone();
        post_json(
            pool,
            "/auth/register",
            json!({"email": "replay@example.com", "password": "password123"}),
        )
        .await;
        let login_res = post_json(
            p2,
            "/auth/login",
            json!({"email": "replay@example.com", "password": "password123"}),
        )
        .await;
        let body = axum::body::to_bytes(login_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let tokens: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let original_refresh = tokens["refresh_token"].as_str().unwrap().to_string();

        // First use succeeds
        let res1 = post_json(
            p3,
            "/auth/refresh",
            json!({"refresh_token": original_refresh}),
        )
        .await;
        assert_eq!(res1.status(), StatusCode::OK);

        // Replay with the same token is rejected
        let res2 = post_json(
            p4,
            "/auth/refresh",
            json!({"refresh_token": original_refresh}),
        )
        .await;
        assert_eq!(res2.status(), StatusCode::UNAUTHORIZED);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml "refresh_returns_new_token_pair|refresh_token_is_single_use" 2>&1 | tail -15
```

Expected: `refresh_returns_new_token_pair` fails because the current handler returns only `access_token`. `refresh_token_is_single_use` fails because the second call succeeds instead of returning 401.

- [ ] **Step 3: Rewrite the `refresh` handler**

In `signalnode-api/src/auth/mod.rs`, replace the entire `async fn refresh` function:

```rust
async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> impl IntoResponse {
    let claims = match token::decode_refresh_token(&body.refresh_token, &state.jwt_secret) {
        Ok(c) => c,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let jti: Uuid = match claims.jti.as_deref().and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "failed to begin refresh transaction");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Atomically consume the old jti — if it was already used (deleted), this returns 0 rows.
    let deleted = sqlx::query("DELETE FROM refresh_tokens WHERE jti = $1")
        .bind(jti)
        .execute(&mut *tx)
        .await;

    match deleted {
        Ok(r) if r.rows_affected() == 0 => return StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "database error consuming refresh token jti");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Ok(_) => {}
    }

    let (new_refresh_token, new_jti, new_expires_at) =
        match token::encode_refresh_token(&claims.sub, &state.jwt_secret) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = ?e, "refresh token encoding failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

    if let Err(e) = sqlx::query(
        "INSERT INTO refresh_tokens (jti, user_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(new_jti)
    .bind(user_id)
    .bind(new_expires_at)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(error = ?e, "failed to persist new refresh token");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "failed to commit refresh transaction");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let access_token = match token::encode_access_token(&claims.sub, &state.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = ?e, "access token encoding failed during refresh");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    Json(AuthResponse {
        access_token,
        refresh_token: new_refresh_token,
    })
    .into_response()
}
```

- [ ] **Step 4: Run all tests and verify they pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -20
```

Expected: 122 tests pass (120 from Task 3 + 2 new). The existing `refresh_valid_token_returns_access_token` test still passes because it only asserts `json["access_token"].is_string()` — the additional `refresh_token` field in the response is benign.

- [ ] **Step 5: Run the core tests to confirm no regression**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml 2>&1 | tail -10
```

Expected: 29 tests pass.

- [ ] **Step 6: Commit**

```bash
git add signalnode-api/src/auth/mod.rs
git commit -m "feat(api): single-use refresh tokens with atomic rotation and replay protection"
```

---

## Self-Review

**Spec coverage:**

| Requirement | Task |
|---|---|
| Refresh tokens stored in DB | Task 1 (migration) + Task 3 (login persists) |
| Single-use: token consumed on refresh | Task 4 (DELETE on use) |
| Replay rejected with 401 | Task 4 (`rows_affected == 0` check) |
| New refresh token issued on rotate | Task 4 (INSERT new jti, return in AuthResponse) |
| JWT signature + expiry still validated | Task 4 (existing `decode_refresh_token` call preserved) |
| Old tokens without jti rejected | Task 4 (`jti.is_none()` → 401) |
| No new dependencies | All tasks (uuid, sqlx, chrono all pre-existing) |

**Placeholder scan:** No TBDs, no "handle edge cases", no "similar to Task N" — all steps contain full code.

**Type consistency:**
- `encode_refresh_token` returns `(String, Uuid, DateTime<Utc>)` — used as `(refresh_token, refresh_jti, refresh_expires_at)` in Task 3 and `(new_refresh_token, new_jti, new_expires_at)` in Task 4. ✓
- `Claims.jti: Option<String>` — extracted with `.as_deref().and_then(|s| s.parse().ok())` → `Uuid` in Task 4. ✓
- `AuthResponse { access_token, refresh_token }` — returned by both `login` and `refresh` after Task 4. ✓
- `sqlx::query(...).bind(refresh_jti: Uuid).bind(user_id: Uuid).bind(refresh_expires_at: DateTime<Utc>)` — sqlx feature flags `uuid` and `chrono` are already enabled. ✓
