# Phase 4 Handoff — Refresh Token Rotation + Replay Protection

**Date:** 2026-05-18
**Branch:** `main` (pushed to `origin/main`, commit range `1ee2b61`–`ce09452`)
**Tests:** 122 API / 29 core — all green (151 total)

---

## What Shipped

Single-use DB-backed refresh tokens with atomic rotation and replay protection.

| Commit | Change |
|--------|--------|
| `1ee2b61` | `refresh_tokens` table: `jti UUID PK`, `user_id FK → users`, `expires_at`, `created_at`; `user_id` index |
| `571961d` | `Claims.jti: Option<String>` (`#[serde(default)]`); `encode_refresh_token` now returns `(String, Uuid, DateTime<Utc>)` |
| `11c2e91` | `decode_refresh_token` enforces jti presence (returns `InvalidToken` if `jti.is_none()`); `refresh_token_has_jti` test asserts UUID format |
| `503a5d6` | `login` persists jti to `refresh_tokens` after issuing token; `login_persists_refresh_token_in_db` test |
| `0b47b0f` | `refresh` handler: atomic DELETE old jti → INSERT new jti in one tx → return `AuthResponse` (both tokens) |
| `0db95d2` | DELETE predicate hardened: `WHERE jti = $1 AND user_id = $2 AND expires_at > NOW()`; tests assert token distinctness |
| `43fe9f4` | Restore timing-attack comment in `login`; note cleanup debt in migration |
| `ce09452` | Plan doc: `docs/superpowers/plans/2026-05-18-refresh-token-rotation.md` |

---

## Security Property Delivered

Refresh tokens are now **single-use**. On each `/auth/refresh` call:

1. JWT signature + expiry validated (`jsonwebtoken`)
2. `jti` claim extracted and verified non-None (`decode_refresh_token`)
3. `DELETE FROM refresh_tokens WHERE jti = $1 AND user_id = $2 AND expires_at > NOW()` — if 0 rows affected → 401 (replay)
4. New jti generated and INSERTed in the same Postgres transaction
5. Both new access token and new refresh token returned

A stolen refresh token is useless the moment the legitimate owner next uses it. The PostgreSQL row lock on DELETE makes the rotation race-free.

---

## Architecture State (post-Phase 4)

`signalnode-api/src/auth/`:

```
token.rs    — Claims { sub, exp, kind, jti: Option<String> }
              encode_access_token → Result<String, _>
              encode_refresh_token → Result<(String, Uuid, DateTime<Utc>), _>
              decode_access_token  — enforces kind = "access"
              decode_refresh_token — enforces kind = "refresh" AND jti.is_some()

mod.rs      — login:   Argon2id verify → encode tokens → INSERT jti → 201
              refresh: decode JWT → DELETE old jti (401 if 0 rows) → INSERT new jti → return AuthResponse
```

`refresh_tokens` table (migration `20260518000012`):

```sql
jti        UUID PRIMARY KEY
user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE
expires_at TIMESTAMPTZ NOT NULL
created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
INDEX refresh_tokens_user_id_idx ON (user_id)
```

---

## Known Technical Debt (updated)

| Item | Severity | Notes |
|---|---|---|
| `refresh_tokens` grows indefinitely | Medium | Expired rows never purged. Needs `DELETE FROM refresh_tokens WHERE expires_at < NOW()` on a schedule. Noted in migration. |
| No rate limiting on auth endpoints | High | `/auth/login` and `/auth/register` unprotected against credential stuffing. **Next phase.** |
| No structured error response bodies | Medium | Errors return bare status codes. Clients can't distinguish error types. |
| No account lockout after failed logins | Medium | Requires HITL review before implementing. |
| Incident logic duplicated between `checker.rs` and `signalnode-api/src/check_result/mod.rs` | Medium | Extract to `signalnode-shared` crate (Phase 5+). |
| `check_membership` / `check_owner` duplicated across three API modules | Low | Extraction deferred. |
| Unused imports in `monitor/mod.rs` and `notification_channel/mod.rs` | Low | Pre-existing warnings. |

---

## Test Counts (as of 2026-05-18 Phase 4)

| Crate | Tests | Status |
|-------|-------|--------|
| signalnode-api | 122 | all pass |
| signalnode-core | 29 | all pass |

**Test commands:**
```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml

DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

---

## Recommended Next Phase: Auth Rate Limiting (`tower_governor`)

**Priority:** High security debt — `/auth/login` and `/auth/register` are unprotected against credential stuffing.

**Approach:**
- Add `tower_governor` (or `axum-governor`) as a middleware on the auth router
- Key by remote IP
- Suggested limits: `/auth/login` — 10 req/min per IP; `/auth/register` — 5 req/min per IP
- Returns `429 Too Many Requests` on breach
- In-memory governor state (no Redis needed for Phase 5)

**First step:** Add `tower_governor = "0.4"` (or latest) to `signalnode-api/Cargo.toml`, then wrap the auth router in `GovernorLayer`.

Read `signalnode-api/src/lib.rs` and `signalnode-api/src/auth/mod.rs` before starting — the router assembly point is in `lib.rs`.

---

## Project Docs

- Domain glossary: `CONTEXT.md`
- Architecture decisions: `docs/adr/`
- Phase 1 PRD: `docs/prd-phase1.md`
- Phase 3 handoff: `docs/phase3-handoff.md`
- Phase 4 plan: `docs/superpowers/plans/2026-05-18-refresh-token-rotation.md`
