# Phase 9 — Per-Module Structured Error Responses

**Date:** 2026-05-20
**Scope:** `signalnode-api` — workspace, monitor, incident, notification_channel, check_result modules

## Problem

Auth errors are structured (Phase 8). The remaining five API modules return bare `StatusCode` with no body. Clients receive 403, 404, 409, 422, or 500 with empty responses and cannot distinguish error types programmatically.

## Goal

Add structured JSON error bodies to all non-auth error paths. Body shape matches Phase 8:

```json
{"code": "not_found", "message": "The requested resource was not found"}
```

HTTP status codes are unchanged. No new dependencies. No schema changes.

## Shared Body Construction

A small private helper avoids duplicating `Json(json!({...}))` construction across five modules. Added to `lib.rs` (already the home of `AppState`):

```rust
#[derive(Serialize)]
pub struct ErrorBody {
    code: &'static str,
    message: Cow<'static, str>,
}
```

Json(ErrorBody { ... })

## Per-Module Error Enums

Each module gets an `error.rs` file with its own enum and `IntoResponse` impl.

### Variant Map

| Module | Variants |
|---|---|
| `workspace` | `SlugTaken`, `InvalidInput(String)`, `Internal` |
| `monitor` | `Forbidden`, `NotFound`, `InvalidInput(String)`, `Internal` |
| `incident` | `Forbidden`, `NotFound`, `Internal` |
| `notification_channel` | `Forbidden`, `NotFound`, `InvalidInput(String)`, `Internal` |
| `check_result` | `Forbidden`, `NotFound`, `InvalidInput(String)`, `Internal` |

### Code/Status/Message Table

| Variant | Status | `code` | `message` |
|---|---|---|---|
| `Forbidden` | 403 | `forbidden` | `You do not have access to this resource` |
| `NotFound` | 404 | `not_found` | `The requested resource was not found` |
| `SlugTaken` | 409 | `slug_taken` | `A workspace with that slug already exists` |
| `InvalidInput(msg)` | 422 | `invalid_input` | _(caller-supplied safe string)_ |
| `Internal` | 500 | `internal_error` | `An internal error occurred` |

`Internal` logs the context via `tracing::error!` before returning the generic body.

## Helper Function Signature Change

`check_membership`, `check_owner`, and `resolve_monitor` (in check_result) currently return `Result<(), StatusCode>`. They change to `Result<(), ModuleError>` (e.g. `Result<(), MonitorError>`). These are private functions — no public API surface change. Callers change from:

```rust
if let Err(status) = check_membership(...).await {
    return status.into_response();
}
```

to:

```rust
if let Err(e) = check_membership(...).await {
    return e.into_response();
}
```

## Module-by-Module Error Mapping

### workspace/mod.rs

| Situation | Old | New |
|---|---|---|
| Empty name or invalid slug | `StatusCode::UNPROCESSABLE_ENTITY` | `WorkspaceError::InvalidInput(...)` |
| Slug already taken (23505) | `StatusCode::CONFLICT` | `WorkspaceError::SlugTaken` |
| DB error | `StatusCode::INTERNAL_SERVER_ERROR` | `WorkspaceError::Internal` |

### monitor/mod.rs

| Situation | Old | New |
|---|---|---|
| Not a workspace member | `StatusCode::FORBIDDEN` / `NOT_FOUND` (from helper) | `MonitorError::Forbidden` / `MonitorError::NotFound` |
| Empty name/url, invalid interval | `StatusCode::UNPROCESSABLE_ENTITY` | `MonitorError::InvalidInput(...)` |
| Monitor not found | `StatusCode::NOT_FOUND` | `MonitorError::NotFound` |
| DB error | `StatusCode::INTERNAL_SERVER_ERROR` | `MonitorError::Internal` |

### incident/mod.rs

| Situation | Old | New |
|---|---|---|
| Not a workspace member | `StatusCode::FORBIDDEN` / `NOT_FOUND` (from helper) | `IncidentError::Forbidden` / `IncidentError::NotFound` |
| DB error | `StatusCode::INTERNAL_SERVER_ERROR` | `IncidentError::Internal` |

### notification_channel/mod.rs

| Situation | Old | New |
|---|---|---|
| Not workspace owner | `StatusCode::FORBIDDEN` / `NOT_FOUND` (from helper) | `NotificationChannelError::Forbidden` / `::NotFound` |
| Empty name/config | `StatusCode::UNPROCESSABLE_ENTITY` | `NotificationChannelError::InvalidInput(...)` |
| Channel not found | `StatusCode::NOT_FOUND` | `NotificationChannelError::NotFound` |
| DB error | `StatusCode::INTERNAL_SERVER_ERROR` | `NotificationChannelError::Internal` |

### check_result/mod.rs

| Situation | Old | New |
|---|---|---|
| Not workspace member | `StatusCode::FORBIDDEN` / `NOT_FOUND` (from helper) | `CheckResultError::Forbidden` / `::NotFound` |
| Monitor not found | `StatusCode::NOT_FOUND` | `CheckResultError::NotFound` |
| Invalid status value | `StatusCode::UNPROCESSABLE_ENTITY` | `CheckResultError::InvalidInput(...)` |
| DB error | `StatusCode::INTERNAL_SERVER_ERROR` | `CheckResultError::Internal` |

## Tests

1–2 body-contract tests per module. Existing status-only tests are untouched.

| Module | Test(s) |
|---|---|
| workspace | `create_workspace_duplicate_slug_returns_structured_error` |
| monitor | `create_monitor_empty_name_returns_structured_error` |
| incident | `list_incidents_forbidden_returns_structured_error` |
| notification_channel | `create_channel_empty_name_returns_structured_error` |
| check_result | `create_check_result_invalid_status_returns_structured_error` |

Each test asserts: correct HTTP status + body contains `"code"` key with expected value.

## Files Changed

| File | Change |
|---|---|
| `src/lib.rs` | Add `pub struct ErrorBody` with `Cow<'static, str>` fields + `Serialize` |
| `src/workspace/error.rs` | New — `WorkspaceError` enum + `IntoResponse` |
| `src/workspace/mod.rs` | `mod error; use error::WorkspaceError;` — replace bare StatusCode returns; add 1 body test |
| `src/monitor/error.rs` | New — `MonitorError` enum + `IntoResponse` |
| `src/monitor/mod.rs` | Replace bare StatusCode; update `check_membership`/`check_owner` return types; add 1 body test |
| `src/incident/error.rs` | New — `IncidentError` enum + `IntoResponse` |
| `src/incident/mod.rs` | Replace bare StatusCode; update `check_membership` return type; add 1 body test |
| `src/notification_channel/error.rs` | New — `NotificationChannelError` enum + `IntoResponse` |
| `src/notification_channel/mod.rs` | Replace bare StatusCode; update helper return types; add 1 body test |
| `src/check_result/error.rs` | New — `CheckResultError` enum + `IntoResponse` |
| `src/check_result/mod.rs` | Replace bare StatusCode; update helper return types; add 1 body test |

## Out of Scope

- Consolidating duplicated `check_membership`/`check_owner` into a shared module — tracked as separate security debt item
- Body assertions on all existing tests — existing tests prove status codes; body contract proven by new tests
- Auth module (`auth/error.rs`) — unchanged
