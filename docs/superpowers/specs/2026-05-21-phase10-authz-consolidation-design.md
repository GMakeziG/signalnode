# Phase 10: Authorization Helper Consolidation

**Date:** 2026-05-21
**Status:** Approved

## Problem

`check_membership` is duplicated verbatim across four `signalnode-api` modules (`monitor`, `incident`, `notification_channel`, `check_result`). `check_owner` is duplicated across two (`monitor`, `notification_channel`). The only difference between copies is the concrete error type returned. The `Forbidden`, `NotFound`, and `Internal` response bodies produced by all four module error enums are identical in status code, `code` string, and message.

## Approach

Introduce a new `signalnode-api/src/authz.rs` module containing:

- `pub enum AuthzError { Forbidden, NotFound, Internal }` with `IntoResponse`
- `pub async fn check_membership(pool, workspace_id, user_id) -> Result<(), AuthzError>`
- `pub async fn check_owner(pool, workspace_id, user_id) -> Result<(), AuthzError>`

`AuthzError` implements `IntoResponse` directly using the same `ErrorBody` pattern established in Phase 9. No `From<AuthzError>` conversions, no new traits, no new crate.

`authz.rs` is strictly scoped to workspace membership/ownership authorization. No validation logic, no resource lookup helpers, nothing unrelated.

## Module Structure

**New:** `signalnode-api/src/authz.rs`

```
pub enum AuthzError { Forbidden, NotFound, Internal }
impl IntoResponse for AuthzError
    Forbidden  → 403  code="forbidden"   message="You do not have access to this resource"
    NotFound   → 404  code="not_found"   message="The requested resource was not found"
    Internal   → 500  code="internal_error" message="An internal error occurred"

pub async fn check_membership(pool: &PgPool, workspace_id: Uuid, user_id: Uuid)
    → Ok(())                   — user is a workspace member
    → Err(Forbidden)           — user exists but is not a member, workspace exists
    → Err(NotFound)            — workspace does not exist
    → Err(Internal)            — DB error

pub async fn check_owner(pool: &PgPool, workspace_id: Uuid, user_id: Uuid)
    → Ok(())                   — user is a member with role = "owner"
    → Err(Forbidden)           — user is a member but not owner, or is not a member but workspace exists
    → Err(NotFound)            — workspace does not exist
    → Err(Internal)            — DB error
```

**Modified:** `signalnode-api/src/lib.rs` — add `pub mod authz;`

## Call Site Changes

Each module replaces its local private helper(s) with a `use crate::authz;` import. The early-return pattern is unchanged:

```rust
// before
if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
    return e.into_response();
}

// after
if let Err(e) = authz::check_membership(&state.pool, workspace_id, current_user.id).await {
    return e.into_response();
}
```

Handler logic is not modified beyond the import and call-site rename.

| Module                 | Deletes                          | Adds                |
|------------------------|----------------------------------|---------------------|
| `monitor`              | `check_membership`, `check_owner` | `use crate::authz;` |
| `incident`             | `check_membership`               | `use crate::authz;` |
| `notification_channel` | `check_membership`, `check_owner` | `use crate::authz;` |
| `check_result`         | `check_membership`               | `use crate::authz;` |

## SQL Queries (preserved verbatim)

`check_membership`:
```sql
SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2)
-- if false: SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)
```

`check_owner`:
```sql
SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2
-- if None: SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)
```

## Testing

**New:** `authz.rs` `#[cfg(test)]` block — 7 integration test cases against a real `PgPool`:

| Helper | Scenario | Expected |
|---|---|---|
| `check_membership` | user is member | `Ok(())` |
| `check_membership` | user not member, workspace exists | `Err(Forbidden)` |
| `check_membership` | workspace does not exist | `Err(NotFound)` |
| `check_owner` | user is owner | `Ok(())` |
| `check_owner` | user is member, not owner | `Err(Forbidden)` |
| `check_owner` | user not member, workspace exists | `Err(Forbidden)` |
| `check_owner` | workspace does not exist | `Err(NotFound)` |

**Unchanged:** all 138 existing `signalnode-api` tests, including Phase 9 body-contract tests. No duplicate `AuthzError` body tests — the response shape is already covered by existing module tests.

## Out of Scope

- `signalnode-shared` crate — future work, not needed here
- `From<AuthzError>` impls on module error enums
- Any refactoring of `resolve_monitor` or other resource-lookup helpers
- Changes to handler logic beyond the import and call-site rename
