# Monitor Foundation

**Date:** 2026-05-14
**Scope:** Migration, workspace-scoped CRUD routes protected by auth + membership, DB-backed tests. No check loop, incidents, alerting, or monitor update/delete.

---

## Schema

### Migration: `20260514000004_monitors.sql`

```sql
CREATE TABLE monitors (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         TEXT        NOT NULL,
    url          TEXT        NOT NULL,
    interval_secs INT        NOT NULL CHECK (interval_secs > 0),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

- `workspace_id` FK with `ON DELETE CASCADE` — deleting a workspace removes its monitors.
- `CHECK (interval_secs > 0)` — DB-level guard; application-level check mirrors this.
- No `owner_id` — ownership flows through workspace membership, not per-monitor.

---

## Module Structure

New file: `signalnode-api/src/monitor/mod.rs`, following the `workspace` module pattern.
Registered in `lib.rs` as a nested router under `/api`, behind existing `auth_middleware`.

---

## Routes

Both routes require a valid Bearer access token (existing `auth_middleware`).

### `POST /api/workspaces/:workspace_id/monitors`

- **Body:** `{ "name": "My Monitor", "url": "https://example.com", "interval_secs": 60 }`
- **Validation:**
  - `workspace_id` path param — Axum `Path<Uuid>` extractor; non-UUID returns 400
  - `name` must be non-empty
  - `url` must be non-empty (format validation deferred)
  - `interval_secs` must be > 0 (i.e., >= 1)
- **Membership check:** `SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2`
  - Row missing: check if workspace exists → 404 if not, 403 if exists but not member
- **Success:** `201 Created` + `{ id, workspace_id, name, url, interval_secs, created_at }`
- **Errors:** 403 not member, 404 workspace not found, 422 invalid body, 500 DB

### `GET /api/workspaces/:workspace_id/monitors`

- **Membership check:** same as above
- **Query:** `SELECT id, workspace_id, name, url, interval_secs, created_at FROM monitors WHERE workspace_id = $1 ORDER BY created_at ASC`
- **Success:** `200 OK` + array (empty array if none)
- **Errors:** 403 not member, 404 workspace not found, 500 DB

---

## Membership Guard

Private async function called at the top of each handler:

```rust
async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode>
```

Logic:
1. Query `workspace_members` for `(workspace_id, user_id)`.
2. If row found → `Ok(())`.
3. If no row → query `workspaces` for `workspace_id`.
   - Workspace exists → `Err(StatusCode::FORBIDDEN)` (403).
   - Workspace not found → `Err(StatusCode::NOT_FOUND)` (404).
4. DB error → `tracing::error!` + `Err(StatusCode::INTERNAL_SERVER_ERROR)`.

---

## Error Handling

- 400 — invalid UUID in path (Axum extractor rejection)
- 403 — authenticated but not a member of the workspace
- 404 — workspace does not exist
- 422 — invalid body (empty name/url, interval_secs < 1)
- 500 — unexpected DB error; logged with `tracing::error!`

---

## Tests

All DB-backed tests use `#[sqlx::test(migrations = "../migrations")]`. Auth-rejection tests use `PgPool::connect_lazy` (no DB required).

Test helpers:
- `create_test_user(pool)` — reuse existing pattern
- `create_test_workspace(pool, user_id)` — inserts workspace + owner membership, returns `workspace_id`

| Test | Type | Assertion |
|---|---|---|
| `create_monitor_success` | DB | 201, response JSON has correct fields |
| `create_monitor_not_member` | DB | 403 — valid workspace, caller not a member |
| `create_monitor_workspace_not_found` | DB | 404 — workspace UUID doesn't exist |
| `create_monitor_invalid_body` | DB | 422 — empty name, empty url, interval_secs = 0 |
| `list_monitors_returns_workspace_monitors` | DB | Only returns monitors for the target workspace; cross-workspace isolation check |
| `list_monitors_empty` | DB | 200 + `[]` when no monitors exist |
| `list_monitors_not_member` | DB | 403 — valid workspace, caller not a member |
| `list_monitors_workspace_not_found` | DB | 404 — workspace UUID doesn't exist |
| `create_monitor_unauthenticated` | Unit | 401 |
| `list_monitors_unauthenticated` | Unit | 401 |

---

## Out of Scope

- Monitor update (PUT/PATCH) or delete
- Monitor lifecycle states (`active`, `paused`, `archived`)
- `kind` field (uptime/ssl/api) — deferred until check loop is designed
- `failure_threshold` / `recovery_threshold` — deferred until Incident logic
- CheckResult recording
- Incidents and alerting
- Background check execution loop
- URL format validation
