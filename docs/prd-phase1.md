## Problem Statement

There is no working monitoring system yet. A user cannot create a Monitor, have it run on a schedule, see whether their target is up or down, or receive an alert when it goes down. The domain model has been defined, the codebase scaffolded, and ADRs recorded — but no executable code exists. Phase 1 must deliver one complete, working end-to-end flow before any supporting infrastructure is added around it.

The environment is internal and DoD/CMMC-aware: access control, audit trail, and secure defaults are non-negotiable from day one.

---

## Solution

Deliver a single tracer-bullet slice that covers every layer of the stack:

1. A User authenticates and creates a Workspace
2. The User creates an HTTP (uptime) Monitor with a check interval, failure threshold, and recovery threshold
3. The User attaches a NotificationChannel (email or webhook) to the Monitor
4. `signalnode-core` picks up the active Monitor, runs the HTTP check on schedule, and writes a CheckResult
5. `signalnode-core` evaluates consecutive CheckResults against the Monitor's thresholds and opens an Incident when the failure threshold is crossed
6. When an Incident opens (or closes), `signalnode-core` dispatches a Notification to each linked NotificationChannel and persists the delivery record
7. `signalnode-api` exposes all entities — Monitors, CheckResults, Incidents, NotificationChannels, Notifications — via a REST API
8. A minimal read-only web view displays Monitor list and current health (derived from open Incidents)

No feature is built in isolation. Every layer must participate in the slice before the next capability is added.

---

## User Stories

1. As a User, I want to register an account so that I can log in and manage my Monitors.
2. As a User, I want to log in with my credentials so that I receive a session token for subsequent API requests.
3. As a User, I want to create a Workspace so that I have an organizational unit to own my Monitors and NotificationChannels.
4. As a Workspace Owner, I want to invite another User to my Workspace so that my team can share access to Monitors.
5. As a Workspace Member, I want to view all Monitors in my Workspace so that I can see what is being tracked.
6. As a Workspace Member, I want to create an HTTP (uptime) Monitor with a target URL, check interval, failure threshold, and recovery threshold so that my site is checked automatically.
7. As a Workspace Member, I want to pause a Monitor so that checks stop temporarily without losing its configuration or history.
8. As a Workspace Member, I want to archive a Monitor so that it is retired but its CheckResult and Incident history is preserved.
9. As a Workspace Member, I want to view the CheckResult history for a Monitor so that I can see a record of past executions.
10. As a Workspace Member, I want to see the current health of a Monitor so that I know immediately whether it is up, degraded, or down.
11. As a Workspace Member, I want to view all open Incidents so that I know which Monitors are currently failing.
12. As a Workspace Member, I want to view past (closed) Incidents with their duration so that I can audit historical downtime.
13. As a Workspace Member, I want to create an email NotificationChannel so that I receive alerts in my inbox when an Incident opens.
14. As a Workspace Member, I want to create a webhook NotificationChannel so that my team's tooling receives alert payloads automatically.
15. As a Workspace Member, I want to attach one or more NotificationChannels to a Monitor so that the right people are notified when that Monitor fails.
16. As a Workspace Member, I want to see the Notification delivery log for an Incident so that I can confirm alerts were dispatched and diagnose missed notifications.
17. As a User, I want `degraded` CheckResults (e.g. high latency, approaching SSL expiry) to appear in the CheckResult history without triggering an Incident, so that I have early visibility of potential issues without false alarms.
18. As a User, I want the system to automatically close an Incident when the Monitor's recovery threshold of consecutive successful CheckResults is met, so that I am not left with stale open Incidents after recovery.
19. As a User, I want to receive a recovery Notification when an Incident closes so that I know the issue is resolved.
20. As a Workspace Owner, I want only Owners to be able to delete Monitors, archive Monitors, or remove NotificationChannels, so that Members cannot accidentally destroy monitoring configuration.
21. As a User, I want all API communication to occur over TLS so that credentials and monitoring data are not transmitted in plaintext.
22. As an auditor, I want every Notification attempt (successful or failed) to be recorded with a timestamp so that there is an immutable delivery audit trail.

---

## Implementation Decisions

### Tracer-bullet scope

Phase 1 implements exactly one Monitor kind: `uptime` (HTTP GET check). The `ssl` and `api` kinds are named in the domain (the `kind` field exists) but their Executor implementations are deferred to Phase 2. The Executor is structured so that new kinds are added without changing the scheduler or evaluator.

### Process architecture (ADR-0001)

`signalnode-core` and `signalnode-api` are deployed as separate processes sharing a single PostgreSQL database. The core engine polls for active Monitors, executes checks, and writes results directly to the database. The API reads and writes the same database to serve the REST surface. No message queue is introduced in Phase 1.

### Database schema (key tables)

- `workspaces` — id, name, created_at
- `users` — id, email, password_hash, created_at
- `workspace_members` — user_id, workspace_id, role (`owner` | `member`)
- `monitors` — id, workspace_id, kind, config (jsonb), status (`active` | `paused` | `archived`), interval_seconds, failure_threshold, recovery_threshold, created_at
- `monitor_notification_channels` — monitor_id, notification_channel_id (join table)
- `check_results` — id, monitor_id, status (`up` | `degraded` | `down`), latency_ms, error_detail, checked_at
- `incidents` — id, monitor_id, opened_at, closed_at (null = open)
- `notification_channels` — id, workspace_id, kind (`email` | `webhook`), config (jsonb, encrypted at rest)
- `notifications` — id, incident_id, notification_channel_id, event (`opened` | `closed`), delivery_status, sent_at

### Monitor health (derived)

A Monitor's current health is not stored. It is derived on read: a Monitor is `down` if it has an open Incident (`incidents.closed_at IS NULL`), `up` otherwise. A database index on `(monitor_id) WHERE closed_at IS NULL` supports efficient dashboard queries.

### IncidentEvaluator logic

After each CheckResult is written, the evaluator:
- If no open Incident exists: counts consecutive trailing `down` results. If count ≥ `failure_threshold`, opens an Incident and triggers NotificationDispatcher with event `opened`.
- If an open Incident exists: counts consecutive trailing `up` results. If count ≥ `recovery_threshold`, closes the Incident and triggers NotificationDispatcher with event `closed`.
- `degraded` results are ignored by the evaluator; they do not count as `up` or `down` for threshold purposes.

### NotificationChannel config security

NotificationChannel `config` (which may contain webhook URLs, SMTP credentials, or API tokens) is encrypted at rest using a server-side key. The key is supplied via environment variable, never stored in the database or committed to source control.

### Authentication

JWT-based. Tokens are short-lived (15 minutes) with refresh tokens. Passwords are hashed with Argon2id. No third-party auth provider in Phase 1 — the system is internal and CMMC-aware, so external OAuth is deferred.

### REST API surface (Phase 1)

```
POST   /auth/register
POST   /auth/login
POST   /auth/refresh

POST   /workspaces
GET    /workspaces/:id
POST   /workspaces/:id/members

POST   /workspaces/:id/monitors
GET    /workspaces/:id/monitors
GET    /workspaces/:id/monitors/:mid
PATCH  /workspaces/:id/monitors/:mid
DELETE /workspaces/:id/monitors/:mid      (archives)

GET    /workspaces/:id/monitors/:mid/check-results
GET    /workspaces/:id/monitors/:mid/incidents
GET    /workspaces/:id/monitors/:mid/incidents/:iid/notifications

POST   /workspaces/:id/notification-channels
GET    /workspaces/:id/notification-channels
DELETE /workspaces/:id/notification-channels/:cid
```

### Minimal web view (stretch goal)

A read-only single-page view listing Monitors and their derived health state. Implemented in `signalnode-web` only after the API is functional. Not a blocker for Phase 1 acceptance.

---

## Testing Decisions

**What makes a good test:** Tests assert external behaviour — inputs and observable outputs — not internal implementation. A test should remain valid after an internal refactor. Prefer integration-level tests where the module boundary is the database or HTTP layer; use unit tests only for pure logic with no I/O.

**Modules to test:**

- **Executor** — Given a mock HTTP server returning specific status codes and latencies, assert that the correct `CheckResult` status and latency are recorded. Test timeout handling (yields `down` with error detail). Test each check kind when implemented.

- **IncidentEvaluator** — Given a sequence of `CheckResult` statuses and a Monitor's threshold configuration, assert whether an Incident is opened, closed, or left unchanged. This is pure logic; test without a real database by operating on in-memory result sequences.

- **NotificationDispatcher** — Given an Incident event and a list of NotificationChannels, assert that a Notification record is written for each channel and that the correct payload is dispatched. Use a mock HTTP server for webhook channels; mock the SMTP client for email.

- **API handlers** — Integration tests against a real PostgreSQL database (test container or local). Assert HTTP status codes, response shapes, and authorization rules (a Member cannot archive a Monitor; a request with an expired JWT is rejected). No mocking of the database layer.

---

## Out of Scope

- `ssl` and `api` Monitor kinds (Executor stubs exist; implementation is Phase 2)
- StatusPage (domain term reserved; no implementation)
- MaintenanceWindow (domain term reserved; no implementation)
- Multi-region check execution
- SaaS billing, subscription management, or plan limits
- Kubernetes deployment (Docker Compose is the Phase 1 target)
- `signalnode-agent` (node/server metrics collection)
- Public-facing dashboard or customer portal
- RBAC beyond the two-role (Owner / Member) model

---

## Further Notes

- The CMMC-aware context means all secrets (JWT signing keys, NotificationChannel credentials, database passwords) must be supplied via environment variables and must never appear in logs, source control, or API responses.
- `tracing` should be used from day one in both `signalnode-core` and `signalnode-api`. Structured logs are the audit trail for CMMC practice CA.2 (Plan of Action) and AU.2 (Audit Events).
- The shared-database coordination model (ADR-0001) means `signalnode-core` must handle the case where the API updates a Monitor's configuration (interval, thresholds, status) between check executions — the scheduler should re-read Monitor config from the database on each tick, not cache it in memory indefinitely.
- Domain glossary: [CONTEXT.md](../CONTEXT.md). Architectural decision: [ADR-0001](../docs/adr/0001-core-and-api-as-separate-processes.md).
