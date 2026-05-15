# Incident Open/Close — Design Spec

**Date:** 2026-05-15  
**Scope:** Incident open/close logic as a CheckResult side-effect + `GET /api/workspaces/{workspace_id}/incidents` (open only).  
**Out of scope:** per-monitor incident history route, Notification dispatch, MaintenanceWindow suppression.

---

## Problem

CheckResults are recorded but there is no observable "is this monitor down?" signal in the API. The Incident entity closes the feedback loop: it marks the period during which a Monitor is unhealthy and makes that period queryable.

---

## Schema

New migration (`20260515000007_incidents.sql`):

```sql
CREATE TABLE incidents (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    monitor_id UUID        NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    opened_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at  TIMESTAMPTZ
);

CREATE INDEX incidents_monitor_id_idx ON incidents (monitor_id, opened_at DESC);
CREATE INDEX incidents_open_idx       ON incidents (monitor_id) WHERE closed_at IS NULL;
```

`closed_at IS NULL` means the Incident is open. The partial index makes the "is there an open incident?" lookup fast. A monitor has at most one open Incident at any time — enforced by evaluation logic, not a DB constraint.

---

## Evaluation Logic

Runs inside the same transaction as the CheckResult insert in `create_check_result`. Evaluation is skipped when the monitor's `status` is not `'active'`.

### Open path

1. Check whether an open Incident already exists for this monitor.  
2. If none: fetch the `failure_threshold` most recent CheckResults ordered by `checked_at DESC`.  
3. If the result count equals `failure_threshold` and every row has `status = 'down'`, insert a new Incident (`closed_at = NULL`).  
4. `degraded` results do not count toward the failure threshold — only `down`.

### Close path

1. Check whether an open Incident exists for this monitor.  
2. If one exists: fetch the `recovery_threshold` most recent CheckResults ordered by `checked_at DESC`.  
3. If the result count equals `recovery_threshold` and every row has `status = 'up'`, set `closed_at = NOW()` on that Incident.

Both paths run inside the same transaction as the CheckResult insert. A DB error in either path rolls back the entire transaction and returns `500` — evaluation failures are never silently swallowed.

---

## Route

### `GET /api/workspaces/{workspace_id}/incidents`

- **Auth:** Bearer access token; caller must be a workspace member.
- **Response:** `200` + JSON array of open Incidents (`closed_at IS NULL`), scoped to the workspace via `monitors.workspace_id`, ordered by `opened_at DESC`.
- **Shape:**
  ```json
  [
    {
      "id": "<uuid>",
      "monitor_id": "<uuid>",
      "opened_at": "<rfc3339>"
    }
  ]
  ```
- **Error cases:** `401` unauthenticated, `403` not a member, `404` workspace not found.
- No pagination in this slice.

Lives in a new module `signalnode-api/src/incident/mod.rs`, wired into `lib.rs`.

---

## Module Placement

```
signalnode-api/src/
  incident/
    mod.rs          ← Incident struct, GET handler, router()
  check_result/
    mod.rs          ← create_check_result now calls evaluation inline
```

The evaluation logic stays inline in `check_result/mod.rs`. It will move to `incident/` once there is a second caller (e.g. the background probe loop in `signalnode-core`).

---

## Tests

All tests are `#[sqlx::test]` integration tests using the existing helper pattern (`create_test_user`, `create_test_workspace`, `create_test_monitor`, `authed`).

### Evaluation tests (in `check_result/mod.rs`)

| Test | Assertion |
|------|-----------|
| `open_incident_after_threshold` | N consecutive `down` results opens exactly one Incident |
| `no_open_below_threshold` | N-1 `down` results does not open |
| `degraded_does_not_count` | A `degraded` result in the streak prevents opening |
| `no_duplicate_open_incident` | Second crossing while Incident is open does not create a second |
| `close_incident_after_recovery` | N consecutive `up` results closes the open Incident |
| `no_close_below_recovery` | N-1 `up` results does not close |
| `paused_monitor_no_open` | CheckResult on a paused monitor does not open an Incident |

### Route tests (in `incident/mod.rs`)

| Test | Assertion |
|------|-----------|
| `get_open_incidents_empty` | Returns `[]` when no open Incidents |
| `get_open_incidents_returns_open_only` | Closed Incidents are excluded |
| `get_open_incidents_scoped_to_workspace` | Incidents from another workspace are excluded |
| `get_open_incidents_ordered_newest_first` | Ordered by `opened_at DESC` |
| `get_open_incidents_not_member` | `403` |
| `get_open_incidents_wrong_workspace` | `404` |
| `get_open_incidents_unauthenticated` | `401` |

---

## Constraints / Decisions

- **No Notification dispatch** in this slice — that is a separate vertical slice.
- **No per-monitor history route** in this slice.
- **No broad refactor** — test helpers remain duplicated; the trigger for extraction (third module) was already documented in the handoff.
- **`kind` field** — evaluation applies to all monitor kinds uniformly for now; kind-specific logic is Phase 2.
- **Concurrency** — two simultaneous CheckResult inserts for the same monitor could theoretically both evaluate the threshold. Acceptable for this slice; serialized at the probe loop level in `signalnode-core`.
