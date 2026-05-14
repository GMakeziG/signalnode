# Monitor CRUD Completion

**Date:** 2026-05-14
**Scope:** Schema fields needed by the downstream scheduler (`status`, `failure_threshold`, `recovery_threshold`, `kind`), GET single monitor, PATCH monitor, DELETE/archive monitor (Owner only), list excluding archived by default. No scheduler, check loop, incidents, notifications, or `signalnode-core` changes.

---

## Schema

### Migration: `20260514000005_monitors_crud_fields.sql`

```sql
ALTER TABLE monitors
    ADD COLUMN status             TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'paused', 'archived')),
    ADD COLUMN failure_threshold  INT  NOT NULL DEFAULT 1
        CHECK (failure_threshold > 0),
    ADD COLUMN recovery_threshold INT  NOT NULL DEFAULT 1
        CHECK (recovery_threshold > 0),
    ADD COLUMN kind               TEXT NOT NULL DEFAULT 'uptime';
```

**Column notes:**

- `status` — scheduler reads this; only `active` monitors are checked. `paused` monitors are skipped without losing history. `archived` is a terminal state set by DELETE; a monitor cannot be re-activated once archived (see State Machine).
- `failure_threshold` — consecutive `down` CheckResults required to open an Incident. Default 1 (alert immediately).
- `recovery_threshold` — consecutive `up` CheckResults required to close an Incident. Default 1.
- `kind` — executor dispatch key. Phase 1 accepts and stores only `'uptime'`. No `CHECK` constraint — Phase 2 will add kinds without needing a migration.
- All columns default to backward-compatible values; existing rows (created before this migration) get `status='active'`, thresholds of `1`, and `kind='uptime'`.

### Full `monitors` table after migration

```
monitors
  id                 UUID        PK  DEFAULT gen_random_uuid()
  workspace_id       UUID        NOT NULL → workspaces(id) ON DELETE CASCADE
  name               TEXT        NOT NULL
  url                TEXT        NOT NULL
  interval_secs      INT         NOT NULL CHECK (> 0)
  status             TEXT        NOT NULL DEFAULT 'active'  CHECK IN ('active','paused','archived')
  failure_threshold  INT         NOT NULL DEFAULT 1         CHECK (> 0)
  recovery_threshold INT         NOT NULL DEFAULT 1         CHECK (> 0)
  kind               TEXT        NOT NULL DEFAULT 'uptime'
  created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()

INDEX monitors_workspace_id_idx ON (workspace_id)
```

---

## Module Structure

No new files. All changes are in `signalnode-api/src/monitor/mod.rs`.

| Change | Detail |
|--------|--------|
| `Monitor` struct | Add `status: String`, `failure_threshold: i32`, `recovery_threshold: i32`, `kind: String` |
| `CreateMonitorRequest` | Add `failure_threshold: Option<i32>` (default 1), `recovery_threshold: Option<i32>` (default 1) |
| Add `PatchMonitorRequest` | All fields `Option<T>` — see Routes |
| Add `check_owner` helper | Returns `Result<(), StatusCode>` — enforces Owner role |
| Add `get_monitor` handler | `GET /api/workspaces/{workspace_id}/monitors/{monitor_id}` |
| Add `patch_monitor` handler | `PATCH /api/workspaces/{workspace_id}/monitors/{monitor_id}` |
| Add `delete_monitor` handler | `DELETE /api/workspaces/{workspace_id}/monitors/{monitor_id}` |
| Update router | Wire three new routes |
| Update `create_monitor` | INSERT + SELECT include new columns; apply threshold defaults |
| Update `list_monitors` | Filter `status != 'archived'` by default; accept `?include_archived=true` |

`lib.rs` requires no changes — the monitor router is already registered.

---

## Status State Machine

```
          PATCH status='paused'
  active ─────────────────────→ paused
  active ←───────────────────── paused
          PATCH status='active'

  active ──→ archived   (DELETE — Owner only)
  paused ──→ archived   (DELETE — Owner only)

  archived → (terminal; no PATCH or further DELETE allowed)
```

- PATCH may only set `status` to `'active'` or `'paused'`. Sending `status: 'archived'` via PATCH → 422.
- PATCH on an archived monitor → 422 regardless of body.
- DELETE on an already-archived monitor → 204 (idempotent no-op; archived is archived).

---

## Routes

### Existing: `POST /api/workspaces/{workspace_id}/monitors` (updated)

**Body changes:**
```json
{
  "name": "My Monitor",
  "url": "https://example.com",
  "interval_secs": 60,
  "failure_threshold": 2,
  "recovery_threshold": 2
}
```

- `failure_threshold` and `recovery_threshold` are optional; default to `1`.
- `kind` is not accepted in the body — Phase 1 hardcodes `'uptime'` in the INSERT.
- `status` is not accepted in the body — always starts as `'active'`.

**Validation additions:**
- `failure_threshold`, if provided, must be `>= 1` → 422 if `< 1`.
- `recovery_threshold`, if provided, must be `>= 1` → 422 if `< 1`.

**Response** — `201 Created`:
```json
{
  "id": "...",
  "workspace_id": "...",
  "name": "My Monitor",
  "url": "https://example.com",
  "interval_secs": 60,
  "status": "active",
  "failure_threshold": 2,
  "recovery_threshold": 2,
  "kind": "uptime",
  "created_at": "...",
  "updated_at": "..."
}
```

---

### Existing: `GET /api/workspaces/{workspace_id}/monitors` (updated)

**Query param:** `?include_archived=true` (optional; default false)

- Default: `WHERE workspace_id = $1 AND status != 'archived'`
- With flag: `WHERE workspace_id = $1`

Response array items gain the same new fields as POST above.

---

### New: `GET /api/workspaces/{workspace_id}/monitors/{monitor_id}`

- **Auth:** Bearer access token
- **Membership check:** `check_membership` (403 not member, 404 workspace not found)
- **Query:** `SELECT ... FROM monitors WHERE id = $1 AND workspace_id = $2`
  - No row → `404` (monitor not found in this workspace; do not reveal existence in other workspaces)
- **Success:** `200 OK` + full monitor JSON (same shape as POST 201 response)
- **Errors:** 401 no token, 403 not member, 404 workspace or monitor not found, 500 DB

---

### New: `PATCH /api/workspaces/{workspace_id}/monitors/{monitor_id}`

- **Auth:** Bearer access token
- **Membership check:** `check_membership` (any member may patch)
- **Body:** all fields optional
  ```json
  {
    "name": "Updated Name",
    "url": "https://new-url.com",
    "interval_secs": 120,
    "status": "paused",
    "failure_threshold": 3,
    "recovery_threshold": 2
  }
  ```
- **Validation:**
  - `name`, if present, must be non-empty → 422
  - `url`, if present, must be non-empty → 422
  - `interval_secs`, if present, must be `>= 1` → 422
  - `status`, if present, must be `'active'` or `'paused'` → 422 for any other value including `'archived'`
  - `failure_threshold`, if present, must be `>= 1` → 422
  - `recovery_threshold`, if present, must be `>= 1` → 422
  - Empty body (all fields `None`) → 422 (nothing to update)
- **Archived guard:** Fetch current `status` before applying; if `'archived'` → `422`
- **SQL:**
  ```sql
  UPDATE monitors
  SET
      name               = COALESCE($1, name),
      url                = COALESCE($2, url),
      interval_secs      = COALESCE($3, interval_secs),
      status             = COALESCE($4, status),
      failure_threshold  = COALESCE($5, failure_threshold),
      recovery_threshold = COALESCE($6, recovery_threshold),
      updated_at         = NOW()
  WHERE id = $7 AND workspace_id = $8
  RETURNING id, workspace_id, name, url, interval_secs, status,
            failure_threshold, recovery_threshold, kind, created_at, updated_at
  ```
  `None` fields bind as SQL `NULL`; `COALESCE` keeps the existing DB value.
- **No row returned** → `404`
- **Success:** `200 OK` + full monitor JSON

---

### New: `DELETE /api/workspaces/{workspace_id}/monitors/{monitor_id}`

- **Auth:** Bearer access token
- **Owner check:** `check_owner` — 403 if Member, 404 if workspace not found
- **Behaviour:** Sets `status = 'archived'`, `updated_at = NOW()`. Hard delete is out of scope.
- **SQL:**
  ```sql
  UPDATE monitors
  SET status = 'archived', updated_at = NOW()
  WHERE id = $1 AND workspace_id = $2
  ```
  - No rows affected → 404 (monitor not found in workspace)
  - Already archived → rows affected = 1, still 204 (idempotent)
- **Success:** `204 No Content` (no body)
- **Errors:** 401, 403 (member or non-member), 404 workspace or monitor not found, 500 DB

---

## Helpers

### `check_membership` (existing — unchanged)

```rust
async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode>
```

No changes. Used by create, list, get-single, patch.

### `check_owner` (new)

```rust
async fn check_owner(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode>
```

Logic:
1. `SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2`
2. Row found, `role = 'owner'` → `Ok(())`
3. Row found, `role = 'member'` → `Err(StatusCode::FORBIDDEN)`
4. No row → check if workspace exists:
   - Exists → `Err(StatusCode::FORBIDDEN)` (authenticated but not a member at all)
   - Not found → `Err(StatusCode::NOT_FOUND)`
5. DB error → `tracing::error!` + `Err(StatusCode::INTERNAL_SERVER_ERROR)`

Used by: `delete_monitor` only.

---

## Router Update

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{workspace_id}/monitors",
            post(create_monitor).get(list_monitors),
        )
        .route(
            "/workspaces/{workspace_id}/monitors/{monitor_id}",
            get(get_monitor).patch(patch_monitor).delete(delete_monitor),
        )
}
```

---

## Error Handling

| Code | Condition |
|------|-----------|
| 401  | Missing or invalid Bearer token (existing middleware) |
| 403  | Authenticated but not a member (or Member attempting Owner-only action) |
| 404  | Workspace not found, or monitor not found in workspace |
| 422  | Validation failure — empty name/url, invalid interval, invalid threshold, invalid status value, `status='archived'` in PATCH, empty PATCH body, PATCH on archived monitor |
| 500  | Unexpected DB error — logged with `tracing::error!` |

---

## Tests

All DB-backed tests use `#[sqlx::test(migrations = "../migrations")]`. Auth-rejection tests use the lazy pool pattern (no DB).

Existing test helpers (`create_test_user`, `create_test_workspace`, `authed`) are reused unchanged.

A new helper is added for convenience:

```rust
async fn create_test_monitor(pool: &PgPool, workspace_id: Uuid) -> Uuid {
    // direct INSERT; returns monitor id
}
```

### `create_monitor` (existing — updated assertions)

| Test | Assertion |
|------|-----------|
| `create_monitor_success` | 201; response includes `status='active'`, `kind='uptime'`, `failure_threshold=1`, `recovery_threshold=1` |
| `create_monitor_with_thresholds` | 201; response reflects explicit `failure_threshold=3`, `recovery_threshold=2` |
| `create_monitor_invalid_threshold` | 422 for `failure_threshold=0` and `recovery_threshold=0` |

(Existing `create_monitor_not_member`, `create_monitor_workspace_not_found`, `create_monitor_invalid_body`, `create_monitor_unauthenticated` remain unchanged.)

### `list_monitors` (existing — updated)

| Test | Assertion |
|------|-----------|
| `list_monitors_excludes_archived` | Archived monitor does not appear in default list |
| `list_monitors_include_archived` | `?include_archived=true` includes the archived monitor |

(Existing list tests remain unchanged.)

### `get_monitor` (new)

| Test | Type | Assertion |
|------|------|-----------|
| `get_monitor_success` | DB | 200; correct JSON for the created monitor |
| `get_monitor_not_found` | DB | 404 — valid workspace, monitor UUID doesn't exist |
| `get_monitor_wrong_workspace` | DB | 404 — monitor exists but belongs to a different workspace |
| `get_monitor_not_member` | DB | 403 |
| `get_monitor_workspace_not_found` | DB | 404 |
| `get_monitor_unauthenticated` | Unit | 401 |

### `patch_monitor` (new)

| Test | Type | Assertion |
|------|------|-----------|
| `patch_monitor_name` | DB | 200; `name` updated, other fields unchanged |
| `patch_monitor_pause` | DB | 200; `status` becomes `'paused'` |
| `patch_monitor_resume` | DB | 200; `status` returns to `'active'` after being paused |
| `patch_monitor_thresholds` | DB | 200; `failure_threshold` and `recovery_threshold` updated |
| `patch_monitor_archived_status_rejected` | DB | 422 — `status: 'archived'` in body |
| `patch_monitor_on_archived` | DB | 422 — monitor is already archived |
| `patch_monitor_empty_body` | DB | 422 — no fields provided |
| `patch_monitor_invalid_interval` | DB | 422 — `interval_secs: 0` |
| `patch_monitor_invalid_threshold` | DB | 422 — `failure_threshold: 0` |
| `patch_monitor_not_found` | DB | 404 |
| `patch_monitor_not_member` | DB | 403 |
| `patch_monitor_unauthenticated` | Unit | 401 |

### `delete_monitor` (new)

| Test | Type | Assertion |
|------|------|-----------|
| `delete_monitor_owner_archives` | DB | 204; subsequent GET returns 404 from default list; `?include_archived=true` shows it with `status='archived'` |
| `delete_monitor_member_forbidden` | DB | 403 — Member (not Owner) cannot archive |
| `delete_monitor_idempotent` | DB | 204 on second DELETE of already-archived monitor |
| `delete_monitor_not_found` | DB | 404 |
| `delete_monitor_workspace_not_found` | DB | 404 |
| `delete_monitor_unauthenticated` | Unit | 401 |

---

## Out of Scope

- `kind` field in `CreateMonitorRequest` — Phase 1 is always `'uptime'`; expose when Phase 2 adds new kinds
- Re-activation of archived monitors via PATCH or any other route
- URL format validation (deferred from Monitor Foundation; still deferred)
- Whitespace-only `name`/`url` (existing data quality debt; unchanged)
- Test helper deduplication (`create_test_user`, `authed`, `TEST_JWT_SECRET`) — extract when a third module needs them
- Workspace member invitation (`POST /api/workspaces/{workspace_id}/members`)
- Anything in `signalnode-core`
