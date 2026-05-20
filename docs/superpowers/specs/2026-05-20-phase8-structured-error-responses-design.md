# Phase 8 — Structured Error Responses (Auth)

**Date:** 2026-05-20
**Scope:** `signalnode-api` auth module only

## Problem

Every error path in `auth/mod.rs` and `middleware.rs` returns a bare `StatusCode` with no body. Clients receive `401`, `409`, `422`, or `500` with empty responses and cannot distinguish error types programmatically.

## Goal

Add structured JSON error bodies to all auth error responses. Body shape:

```json
{"code": "invalid_credentials", "message": "Invalid email or password"}
```

HTTP status codes are unchanged. No new dependencies. No schema changes.

## Error Type

New file: `signalnode-api/src/auth/error.rs`

```rust
pub enum AuthError {
    InvalidCredentials,
    InvalidToken,
    EmailTaken,
    InvalidInput(String),
    Internal,
}
```

`IntoResponse` mapping:

| Variant | Status | `code` | `message` |
|---|---|---|---|
| `InvalidCredentials` | 401 | `invalid_credentials` | `Invalid email or password` |
| `InvalidToken` | 401 | `invalid_token` | `Token is invalid or expired` |
| `EmailTaken` | 409 | `email_taken` | `An account with that email already exists` |
| `InvalidInput(msg)` | 422 | `invalid_input` | _(msg, caller-supplied safe validation string)_ |
| `Internal` | 500 | `internal_error` | `An internal error occurred` |

`AccountLocked` is **not a public variant**. The login handler maps a locked account to `InvalidCredentials` internally, preserving the Phase 6 security policy: clients cannot distinguish a bad password from a locked account.

`Internal` logs the real error via `tracing::error!` before returning the generic body.

## Route Conversion

**`auth/mod.rs`** — all `StatusCode::X.into_response()` calls replaced:

| Handler | Situation | New |
|---|---|---|
| `register` | empty email/password | `AuthError::InvalidInput("Email and password are required".into())` |
| `register` | duplicate email | `AuthError::EmailTaken` |
| `register` | DB error | `AuthError::Internal` |
| `login` | user not found | `AuthError::InvalidCredentials` |
| `login` | account locked | `AuthError::InvalidCredentials` (mapped from locked state) |
| `login` | wrong password | `AuthError::InvalidCredentials` |
| `login` | DB error | `AuthError::Internal` |
| `refresh` | bad JWT / no jti / replayed token | `AuthError::InvalidToken` |
| `refresh` | DB error | `AuthError::Internal` |

**`middleware.rs`** — `AuthExtractor` rejection type changes:

```rust
// before
type Rejection = StatusCode;

// after
type Rejection = AuthError;
```

The two `StatusCode::UNAUTHORIZED` returns in `from_request_parts` become `AuthError::InvalidToken`.

## Tests

Two new body-assertion tests in `auth/mod.rs`:

1. `login_wrong_password_returns_structured_error` — registers user, sends wrong password, asserts `401`, deserializes body, checks `code == "invalid_credentials"` and `message == "Invalid email or password"`.
2. `register_duplicate_email_returns_structured_error` — registers user twice, asserts second response is `409`, checks `code == "email_taken"`.

Existing 164 tests assert only on status codes and are untouched.

## Files Changed

| File | Change |
|---|---|
| `auth/error.rs` | New — `AuthError` enum + `IntoResponse` |
| `auth/mod.rs` | `mod error; use error::AuthError;` — replace ~27 `StatusCode` returns; add 2 body tests |
| `middleware.rs` | `type Rejection = AuthError`; 2 `StatusCode::UNAUTHORIZED` → `AuthError::InvalidToken` |

## Out of Scope

- Other API modules (workspace, monitor, incident, notification_channel, check_result) — follow-on phase
- Global `ApiError` crate type — promote from `AuthError` when other modules adopt the pattern
- Body assertions on all existing tests — existing tests cover status codes; body contract is proven by the two new tests
