# NotificationChannel + Outbox Stub — Design Spec

**Date:** 2026-05-15
**Scope:** NotificationChannel CRUD, transactional outbox (`pending_notifications`), stub dispatch after incident opens. Test helper extraction as prerequisite commit.
**Out of scope:** outbox worker, real email/webhook delivery, retry logic, delivery status, notification templates, MaintenanceWindow suppression.

---

## Problem

Incidents open and close, but nothing tells anyone. The NotificationChannel entity lets workspace owners register delivery addresses (email or webhook). When an Incident opens, the system fans out to all registered channels via a transactional outbox. This slice establishes the full channel-management surface and the outbox write path; the delivery worker is the next slice.

---

## Commits (ordered)

1. **Extract shared test helpers** — `signalnode-api/src/tests/helpers.rs` (no behaviour change, pure refactor)
2. **Migrations** — `notification_channels` + `pending_notifications` tables
3. **NotificationChannel module** — POST + GET + DELETE routes
4. **Outbox fanout** — changes to `create_check_result` + stub dispatch function + outbox integration tests

---

## Schema

### Migration: `notification_channels`

```sql
CREATE TABLE notification_channels (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind         TEXT        NOT NULL CHECK (kind IN ('email', 'webhook')),
    target       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX notification_channels_workspace_id_idx
    ON notification_channels (workspace_id);
```

`target` is the delivery address — an email address for `kind = 'email'`, a URL for `kind = 'webhook'`. Format validation is deferred to the delivery worker. `kind` has a DB CHECK constraint; the set is stable and small.

### Migration: `pending_notifications`

```sql
CREATE TABLE pending_notifications (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    incident_id  UUID        NOT NULL REFERENCES incidents(id) ON DELETE CASCADE,
    channel_kind TEXT        NOT NULL CHECK (channel_kind IN ('email', 'webhook')),
    target       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX pending_notifications_incident_id_idx
    ON pending_notifications (incident_id);
CREATE INDEX pending_notifications_created_at_idx
    ON pending_notifications (created_at);
```

`channel_kind` and `target` are snapshotted at INSERT time so the future worker never needs to re-read `notification_channels`. A deleted channel still receives its in-flight notification. `created_at` is indexed for the worker's poll query (`ORDER BY created_at ASC`). No `status` or `delivered_at` column — the worker adds those via migration when needed.

---

## Routes

New module: `src/notification_channel/mod.rs`. Registered in `lib.rs` as `.nest("/api", notification_channel::router())`.

### `POST /api/workspaces/{workspace_id}/notification-channels`

- **Auth:** Owner-only (`check_owner` — same pattern as monitor DELETE)
- **Body:** `{ "kind": "email" | "webhook", "target": "<non-empty string>" }`
- **Validation:** `kind` not in `{'email', 'webhook'}` → 422; `target` empty → 422
- **Response:** 201 + `{ id, workspace_id, kind, target, created_at }`

### `GET /api/workspaces/{workspace_id}/notification-channels`

- **Auth:** Member (`check_membership` — same pattern as incident GET)
- **Response:** 200 + array ordered `created_at ASC`

### `DELETE /api/workspaces/{workspace_id}/notification-channels/{channel_id}`

- **Auth:** Owner-only
- **Behavior:** Hard delete. `DELETE … WHERE id = $1 AND workspace_id = $2`. `rows_affected = 0` → 404. Else 204. Not idempotent — a hard-deleted row is gone and 404 is the honest response.

---

## Dispatch Integration

### Inside `create_check_result` — open path (inside `tx`)

Two changes to the existing open-incident block:

1. Change the incident INSERT to `RETURNING id` + `fetch_one` to capture `incident_id: Uuid`.
2. Immediately after: query `notification_channels WHERE workspace_id = $1` and INSERT one `pending_notifications` row per channel (snapshotting `channel_kind` and `target`). Both INSERTs are inside `tx` — they commit or roll back atomically.

`workspace_id` is already available from the path params. No extra JOIN needed.

### After `tx.commit()`

If an incident was opened (`Option<Uuid>` carried out of the open path), call:

```rust
notification_channel::dispatch_notifications(&state.pool, incident_id).await;
```

### `dispatch_notifications` (stub)

Defined in `notification_channel/mod.rs` as a `pub async fn`:

1. Query `pending_notifications WHERE incident_id = $1`.
2. For each row: `tracing::info!(channel_kind, target, %incident_id, "stub: notification queued")`.
3. Return — no error propagation. The CheckResult `201` is already committed.

The next slice replaces this function body with real delivery + status tracking. The function signature does not change.

---

## Error Handling

All DB errors: `tracing::error!` + return `500`. Errors are opaque status codes, no body — consistent with existing behaviour. `dispatch_notifications` errors are not propagated; a dispatch failure after a committed CheckResult is a stub-only concern.

---

## Tests

### Commit 1 — test helper extraction

Extract into `src/tests/helpers.rs` (behind `#[cfg(test)]`):

- `create_test_user(pool) -> Uuid`
- `create_test_workspace(pool, user_id) -> Uuid`
- `create_test_monitor(pool, workspace_id) -> Uuid`
- `authed(pool, method, uri, user_id, body) -> Response`
- `TEST_JWT_SECRET: &str`

Remove duplicates from `workspace`, `monitor`, `check_result`, `incident` modules. No behaviour change — compile + test pass is the acceptance criterion.

### Commit 2 — migrations (no application tests)

`sqlx::test` runs migrations automatically; schema correctness is validated by every subsequent integration test.

### Commit 3 — `notification_channel` module (~17 tests)

| Test | Asserts |
|---|---|
| `create_channel_success` | 201; response contains `id` (UUID), `workspace_id`, `kind`, `target`, `created_at`; `kind` and `target` echo request values |
| `create_channel_invalid_kind` | 422 |
| `create_channel_empty_target` | 422 |
| `create_channel_not_member` | 404 (workspace does not exist) |
| `create_channel_member_not_owner` | 403 |
| `create_channel_unauthenticated` | 401 |
| `list_channels_empty` | 200, `[]` |
| `list_channels_member_can_read` | non-owner workspace member receives 200 (confirms GET is member-accessible, not owner-only) |
| `list_channels_ordered_oldest_first` | 200, `created_at ASC` ordering across two channels |
| `list_channels_scoped_to_workspace` | channels from a second workspace not returned |
| `list_channels_not_member` | 403 |
| `list_channels_unauthenticated` | 401 |
| `delete_channel_success` | 204, row absent from DB |
| `delete_channel_not_found` | 404 |
| `delete_channel_wrong_workspace` | 404 |
| `delete_channel_member_not_owner` | 403 |
| `delete_channel_unauthenticated` | 401 |

### Commit 4 — outbox integration (+3 tests in `check_result` module)

| Test | Asserts |
|---|---|
| `pending_notifications_created_when_incident_opens` | Monitor with `failure_threshold = 1`, one channel exists; POST `down` → incident opens → `pending_notifications` count = 1; `channel_kind` and `target` match the channel |
| `no_pending_notifications_when_incident_does_not_open` | Monitor with `failure_threshold = 1`, one channel exists; POST `up` with no open incident → `pending_notifications` count = 0 |
| `no_pending_notifications_when_no_channels` | Monitor with `failure_threshold = 1`, no channels; POST `down` → incident opens, `pending_notifications` count = 0 |

**Running test count: 96 → ~116** (96 + 17 channel tests + 3 outbox tests).

---

## What This Slice Does Not Do

- No outbox worker or polling loop
- No real HTTP POST to webhook URLs
- No real email sending
- No delivery status (`pending` / `delivered` / `failed`) on `pending_notifications`
- No retry logic
- No notification templates or message bodies
- No per-monitor channel filtering (all workspace channels notified on any monitor incident)
