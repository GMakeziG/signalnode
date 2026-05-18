# Phase 6: Account Lockout Design

**Date:** 2026-05-18  
**Status:** Approved  
**Scope:** `/auth/login` only — per-account, time-based lockout after repeated credential failures

---

## Context

Phase 5 added per-IP rate limiting via `tower_governor`. Rate limiting slows brute-force attacks but does not stop a distributed attacker who stays under the per-IP threshold. Account lockout adds a second, orthogonal layer: once a single account receives 10 consecutive failed login attempts (from any source IP), it is locked for 15 minutes regardless of IP.

The two layers have complementary keys — rate limiting is IP-keyed, lockout is account-keyed — and complementary jobs. Neither is removed or modified by this phase.

---

## Scope

- **In scope:** `/auth/login` lockout tracking and enforcement
- **Out of scope:** `/auth/refresh` (protected by rate limiting + token replay protection), `/auth/register` (no credential check against existing accounts), admin unlock API (documented as future work below), append-only audit log of attempt events

---

## Data Model

One new migration adds the `login_attempts` table:

```sql
CREATE TABLE login_attempts (
    user_id        UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    failed_count   INT NOT NULL DEFAULT 0,
    locked_until   TIMESTAMPTZ,           -- NULL means not locked
    last_failed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

**Invariants:**

- One row per user. Rows only exist while there are outstanding failures — deleted on successful login.
- `locked_until` is nullable. `NULL` means not locked. `locked_until > NOW()` means actively locked.
- The locked state is cleared naturally by the success-path `DELETE` — no explicit NULL reset needed.
- No indexes beyond the PK; all lookups are point queries by `user_id`.
- `ON DELETE CASCADE` — user deletion cleans up lockout state automatically.

---

## Login Handler Logic

The `login` handler in `signalnode-api/src/auth/mod.rs` gains two DB operations inserted between the existing user lookup and token issuance steps.

### Updated flow

```
1. SELECT id, password_hash FROM users WHERE email = $1
   → not found: dummy bcrypt verify → 401 (unchanged, timing mitigation)

2. SELECT locked_until FROM login_attempts
   WHERE user_id = $1 AND locked_until > NOW()
   → row returned: return 401 immediately
     NOTE: locked accounts skip dummy verify and skip bcrypt.
     The generic 401 is intentional — do not reveal whether the account
     exists or is locked. Rate limiting makes timing analysis impractical
     at this point, so the missing dummy-verify is an acceptable trade-off.

3. spawn_blocking verify_password(input_password, stored_hash)
   → wrong password:
       INSERT INTO login_attempts (user_id, failed_count, last_failed_at)
       VALUES ($1, 1, NOW())
       ON CONFLICT (user_id) DO UPDATE SET
           failed_count   = login_attempts.failed_count + 1,
           last_failed_at = NOW(),
           locked_until   = CASE
               WHEN login_attempts.failed_count + 1 >= 10
               THEN NOW() + INTERVAL '15 minutes'
               ELSE login_attempts.locked_until
           END
       → 401

   → correct password:
       DELETE FROM login_attempts WHERE user_id = $1
       → issue access + refresh tokens (unchanged)
```

### Key invariants

- The upsert is a single atomic Postgres statement — no TOCTOU race between reading and writing `failed_count`.
- `login_attempts.failed_count + 1` in the `CASE` refers to the pre-update stored value; `+ 1` represents the incoming new value, so the `>= 10` condition fires exactly on the 10th consecutive failure.
- Lockout check runs before bcrypt — locked accounts never pay the bcrypt CPU cost.
- Successful login deletes the row regardless of current `failed_count` — full reset, no partial state.
- The `locked_until` column is not explicitly reset to NULL on success; the row deletion handles cleanup.

---

## Error Handling

All new DB operations follow the existing pattern:

- Unexpected DB errors: `tracing::error!(error = ?e, "...")` + `StatusCode::INTERNAL_SERVER_ERROR`
- No new error types introduced
- The lockout check query and the upsert are each single statements; neither requires a transaction

---

## Testing

Six new `#[sqlx::test]` cases in `signalnode-api/src/auth/mod.rs`:

| Test | What it verifies |
|---|---|
| `login_locked_account_returns_401` | Register user, fail login 10×, assert 11th attempt returns 401 |
| `login_lockout_not_triggered_before_threshold` | Fail 9×, assert 10th attempt still reaches password check (returns 401 for wrong pw, not lockout path) |
| `login_successful_resets_failure_count` | Fail 9×, succeed once, fail again — assert `failed_count = 1` in `login_attempts` |
| `login_success_after_lockout_expires` | Lock account, manually set `locked_until` to a past timestamp via direct DB update, assert next attempt reaches password check |
| `login_wrong_password_increments_count` | Fail 3×, assert `failed_count = 3` in `login_attempts` |
| `login_correct_password_deletes_attempt_row` | Fail 5×, succeed — assert no row in `login_attempts` |

`login_success_after_lockout_expires` manipulates `locked_until` directly via the test pool rather than sleeping — keeps tests fast and deterministic.

All six tests follow the existing `#[sqlx::test(migrations = "../migrations")]` pattern.

---

## Migration Filename

`20260518000013_login_attempts.sql`

---

## Future Work

- **Admin unlock API** — a `DELETE /api/admin/users/:id/lockout` endpoint (or similar) that clears the `login_attempts` row. Requires introducing an admin role and middleware; out of scope for Phase 6.
- **Lockout notification** — email the account owner when their account is locked. Requires the notification system to be wired to auth events.
- **Structured error response bodies** — currently all error paths return bare status codes. A future phase should add `{ "error": "..." }` JSON bodies so clients can give users actionable messages (e.g. distinguishing a locked account from a wrong password in a support context, if the policy changes).
- **`refresh_tokens` table cleanup** — expired rows accumulate indefinitely; needs a periodic `DELETE FROM refresh_tokens WHERE expires_at < NOW()` job (security debt item #2).
