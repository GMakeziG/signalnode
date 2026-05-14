# Workspace + Membership Foundation

**Date:** 2026-05-14  
**Scope:** Migrations, protected CRUD routes, DB-backed tests. No monitors, incidents, or invite flows.

---

## Schema

### Migration: `20260514000002_workspaces.sql`

```sql
CREATE TABLE workspaces (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT        NOT NULL,
    slug       TEXT        NOT NULL UNIQUE,
    owner_id   UUID        NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Migration: `20260514000003_workspace_members.sql`

```sql
CREATE TABLE workspace_members (
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id      UUID        NOT NULL REFERENCES users(id)      ON DELETE CASCADE,
    role         TEXT        NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'member')),
    joined_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (workspace_id, user_id)
);
```

`slug` is globally unique. `role` is constrained to `'owner'` or `'member'` via a CHECK constraint.

---

## Module Structure

New file: `signalnode-api/src/workspace/mod.rs`, following the `auth` module pattern.  
Wired into `lib.rs` under the existing protected router (behind `auth_middleware`).

---

## Routes

Both routes require a valid Bearer access token (existing `auth_middleware`).

### `POST /api/workspaces`

- **Body:** `{ "name": "My Org", "slug": "my-org" }`
- **Validation:**
  - `name` must be non-empty
  - `slug` must match `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` (lowercase alphanumeric + hyphens, no leading or trailing hyphen)
- **Behaviour:** Single transaction — INSERT into `workspaces`, then INSERT into `workspace_members` with `role = 'owner'` and `user_id = current_user.id`
- **Success:** `201 Created` + `{ "id", "name", "slug", "owner_id", "created_at" }`
- **Errors:**
  - `409 Conflict` — slug already taken
  - `422 Unprocessable Entity` — invalid name or slug

### `GET /api/workspaces`

- **Behaviour:** SELECT workspaces WHERE a `workspace_members` row exists for `current_user.id`
- **Success:** `200 OK` + array of `{ "id", "name", "slug", "owner_id", "created_at" }`
- **Empty:** `200 OK` + `[]`

---

## Error Handling

- Slug conflicts → detect `is_unique_violation()` on the `workspaces` INSERT, return 409
- All unexpected DB errors → `tracing::error!` + 500
- Auth failures handled by existing middleware (401)

---

## Tests

All DB-backed tests use `#[sqlx::test(migrations = "../migrations")]`. Auth-only tests (401 cases) use the lazy pool pattern from `lib.rs`.

| Test | Type | Assertion |
|------|------|-----------|
| `create_workspace_success` | DB | 201, response JSON has correct fields |
| `create_workspace_owner_membership` | DB | After create, `workspace_members` row exists with `user_id = creator`, `role = 'owner'` |
| `create_workspace_duplicate_slug` | DB | 409 |
| `create_workspace_invalid_slug` | DB | 422 (uppercase, spaces, leading hyphen, trailing hyphen) |
| `create_workspace_empty_name` | DB | 422 |
| `list_workspaces_returns_own` | DB | Only returns workspaces the authed user is a member of |
| `list_workspaces_empty` | DB | 200 + `[]` |
| `create_workspace_unauthenticated` | Unit | 401 |
| `list_workspaces_unauthenticated` | Unit | 401 |

---

## Out of Scope

- Workspace invitations / adding members
- Monitors and incidents
- Role-based permissions beyond `owner`/`member` presence
- Workspace deletion or update
