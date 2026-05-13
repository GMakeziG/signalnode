#!/usr/bin/env bash
# Usage: GITHUB_TOKEN=<token> bash docs/publish-issues.sh
# Requires: curl, jq
set -euo pipefail

REPO="GMakeziG/signalnode"
API="https://api.github.com/repos/$REPO/issues"
AUTH="Authorization: Bearer $GITHUB_TOKEN"
ACCEPT="Accept: application/vnd.github+json"

create_issue() {
  local title="$1"
  local body="$2"
  local labels="$3"   # comma-separated or empty string
  local payload
  if [ -n "$labels" ]; then
    payload=$(jq -n --arg t "$title" --arg b "$body" --argjson l "$(echo "$labels" | jq -Rc 'split(",")' )" '{title:$t,body:$b,labels:$l}')
  else
    payload=$(jq -n --arg t "$title" --arg b "$body" '{title:$t,body:$b}')
  fi
  curl -sf -X POST "$API" -H "$AUTH" -H "$ACCEPT" -H "Content-Type: application/json" -d "$payload" | jq -r '.number'
}

# Find PRD parent issue
PRD=$(curl -sf "$API?state=open&per_page=50" -H "$AUTH" -H "$ACCEPT" | jq -r '.[] | select(.title | test("PRD.*Phase 1")) | .number' | head -1)
echo "PRD issue: #$PRD"

# ── Issue 1: Bootstrap ────────────────────────────────────────────────────────
I1=$(create_issue \
  "[Phase 1] #1 — Project bootstrap" \
  "## Parent
#$PRD

## What to build
Set up the foundational skeleton that all other slices build on:
- Cargo workspace with \`signalnode-core\` and \`signalnode-api\` crates
- sqlx migration infrastructure (versioned, runnable via \`sqlx migrate run\`)
- Docker Compose with Postgres for local development
- \`GET /health\` endpoint in \`signalnode-api\` returning 200 OK
- \`tracing\` wired into both binaries (structured JSON logs)
- \`.env.example\` with all required environment variable keys (no values)

No business logic. Every subsequent issue assumes this slice is complete and runnable.

## Acceptance criteria
- [ ] \`docker compose up\` starts Postgres and both binaries without error
- [ ] \`GET /health\` returns HTTP 200
- [ ] Migration runner applies an empty initial migration cleanly
- [ ] Both binaries emit structured JSON logs on startup
- [ ] \`.env.example\` documents every env var used (DB URL, JWT secret, etc.)

## Blocked by
None — can start immediately" \
  "ready-for-agent")
echo "Created #$I1: Bootstrap"

# ── Issue 2: Auth (HITL) ─────────────────────────────────────────────────────
I2=$(create_issue \
  "[Phase 1] #2 — Auth: register, login, JWT, refresh (HITL)" \
  "## Parent
#$PRD

## What to build
User authentication end-to-end:
- \`POST /auth/register\` — create User, hash password with Argon2id
- \`POST /auth/login\` — validate credentials, return short-lived JWT (15 min) + refresh token
- \`POST /auth/refresh\` — exchange refresh token for new JWT
- \`users\` DB table
- Middleware that validates JWT on protected routes

HITL: JWT signing key rotation policy, refresh token lifetime, and password complexity requirements need explicit review before merging.

## Acceptance criteria
- [ ] Registration rejects duplicate email
- [ ] Login returns JWT + refresh token on valid credentials; 401 on invalid
- [ ] JWT expiry is enforced — expired token returns 401 on protected routes
- [ ] Refresh token exchanges correctly; used tokens are invalidated (no replay)
- [ ] Passwords are stored as Argon2id hashes — plaintext never logged or returned
- [ ] JWT signing key is supplied via environment variable only
- [ ] Integration tests cover all happy paths and auth failure cases

## Blocked by
- #$I1" \
  "")
echo "Created #$I2: Auth"

# ── Issue 3: Workspace + membership (HITL) ────────────────────────────────────
I3=$(create_issue \
  "[Phase 1] #3 — Workspace + membership: Owner/Member RBAC (HITL)" \
  "## Parent
#$PRD

## What to build
Workspace creation and role-based membership:
- \`POST /workspaces\` — create Workspace; creator becomes Owner
- \`GET /workspaces/:id\` — get Workspace (members only)
- \`POST /workspaces/:id/members\` — Owner invites User by email; assigns role (\`owner\` | \`member\`)
- \`workspaces\` and \`workspace_members\` DB tables
- Middleware enforces Owner-only actions throughout the API

HITL: Ownership transfer, role escalation rules, and what Members vs Owners can see/do need review before merging.

## Acceptance criteria
- [ ] Workspace creator is automatically the Owner
- [ ] Owner can invite another registered User as Member or Owner
- [ ] Member cannot invite users, delete the Workspace, or perform Owner-only actions
- [ ] Non-members receive 403 on all Workspace endpoints
- [ ] Integration tests cover Owner actions, Member restrictions, and non-member rejection

## Blocked by
- #$I2" \
  "")
echo "Created #$I3: Workspace + membership"

# ── Issue 4: Monitor CRUD ─────────────────────────────────────────────────────
I4=$(create_issue \
  "[Phase 1] #4 — Monitor CRUD (uptime kind)" \
  "## Parent
#$PRD

## What to build
Full Monitor lifecycle management for the \`uptime\` kind:
- \`POST /workspaces/:id/monitors\` — create Monitor (kind: uptime, url, interval_seconds, failure_threshold, recovery_threshold)
- \`GET /workspaces/:id/monitors\` — list Monitors in Workspace
- \`GET /workspaces/:id/monitors/:mid\` — get single Monitor
- \`PATCH /workspaces/:id/monitors/:mid\` — update config or status (\`active\` | \`paused\`)
- \`DELETE /workspaces/:id/monitors/:mid\` — archive (Owner only; sets status to \`archived\`, not hard delete)
- \`monitors\` DB table with \`kind\`, \`config\` (jsonb), \`status\`, \`interval_seconds\`, \`failure_threshold\`, \`recovery_threshold\`

Monitor health (up/down) is derived — not stored. Derived from open Incidents in a later slice.

## Acceptance criteria
- [ ] Monitor created with valid uptime config is returned with correct fields
- [ ] Paused Monitor is skipped by scheduler (verified in slice #6)
- [ ] Archived Monitor is excluded from list by default; history preserved
- [ ] Member can create/update/pause; only Owner can archive
- [ ] Invalid interval or threshold values are rejected with 422
- [ ] Integration tests cover CRUD, lifecycle transitions, and auth enforcement

## Blocked by
- #$I3" \
  "ready-for-agent")
echo "Created #$I4: Monitor CRUD"

# ── Issue 5: NotificationChannel CRUD — webhook only (HITL) ───────────────────
I5=$(create_issue \
  "[Phase 1] #5 — NotificationChannel CRUD: webhook kind (HITL)" \
  "## Parent
#$PRD

## What to build
NotificationChannel management for webhook destinations only:
- \`POST /workspaces/:id/notification-channels\` — create channel (kind: webhook, url)
- \`GET /workspaces/:id/notification-channels\` — list channels
- \`DELETE /workspaces/:id/notification-channels/:cid\` — remove channel (Owner only)
- Attach/detach channel from Monitor: \`POST/DELETE /workspaces/:id/monitors/:mid/notification-channels/:cid\`
- \`notification_channels\` table + \`monitor_notification_channels\` join table
- Webhook URL stored encrypted at rest (server-side key from env var)

HITL: Encryption scheme, key management approach, and what happens to attached Monitors when a channel is deleted need review before merging.

## Acceptance criteria
- [ ] Webhook URL is encrypted at rest; plaintext never appears in logs or API responses
- [ ] Channel can be attached to and detached from a Monitor
- [ ] Deleting a channel detaches it from all Monitors cleanly
- [ ] Only Owner can delete a channel
- [ ] Integration tests cover CRUD, attach/detach, and encryption at rest

## Blocked by
- #$I3" \
  "")
echo "Created #$I5: NotificationChannel CRUD"

# ── Issue 6: Core scheduler + uptime Executor ─────────────────────────────────
I6=$(create_issue \
  "[Phase 1] #6 — signalnode-core: scheduler + uptime Executor" \
  "## Parent
#$PRD

## What to build
The \`signalnode-core\` binary polls the DB for active Monitors and executes HTTP uptime checks:
- Scheduler loop: on each tick, fetch all \`active\` Monitors due for a check (based on \`interval_seconds\` + last \`checked_at\`)
- Executor (uptime kind): HTTP GET to target URL; record status (\`up\` / \`degraded\` / \`down\`), latency_ms, error_detail
  - 2xx → \`up\`
  - Timeout or connection refused → \`down\`
  - High latency (configurable threshold) → \`degraded\`
- Write \`CheckResult\` to DB after each execution
- \`check_results\` DB table
- Scheduler re-reads Monitor config from DB each tick — no in-memory caching of config

## Acceptance criteria
- [ ] Active Monitor receives a CheckResult within one interval period of startup
- [ ] Paused Monitor receives no CheckResults
- [ ] Unreachable target URL produces a \`down\` CheckResult with error_detail
- [ ] Slow response above latency threshold produces \`degraded\` CheckResult
- [ ] Executor unit tests: mock HTTP server covering 2xx, 4xx, 5xx, timeout, slow response
- [ ] CheckResults accumulate correctly across multiple scheduler ticks

## Blocked by
- #$I4" \
  "ready-for-agent")
echo "Created #$I6: Scheduler + Executor"

# ── Issue 7: CheckResult API ──────────────────────────────────────────────────
I7=$(create_issue \
  "[Phase 1] #7 — CheckResult API" \
  "## Parent
#$PRD

## What to build
Expose CheckResult history via the REST API:
- \`GET /workspaces/:id/monitors/:mid/check-results\` — paginated list, newest first
- Optional query params: \`status\` filter (\`up\`/\`degraded\`/\`down\`), \`limit\`, \`cursor\`
- Workspace membership required

## Acceptance criteria
- [ ] Returns CheckResults for the specified Monitor newest-first
- [ ] Pagination works correctly across multiple pages
- [ ] Status filter returns only matching results
- [ ] Non-member receives 403
- [ ] Integration tests with pre-seeded CheckResults

## Blocked by
- #$I4
- #$I6" \
  "ready-for-agent")
echo "Created #$I7: CheckResult API"

# ── Issue 8: IncidentEvaluator ────────────────────────────────────────────────
I8=$(create_issue \
  "[Phase 1] #8 — IncidentEvaluator: threshold-based Incident open/close" \
  "## Parent
#$PRD

## What to build
After each CheckResult is written, evaluate whether an Incident should open or close:
- No open Incident: count consecutive trailing \`down\` results. If count ≥ \`failure_threshold\` → open Incident.
- Open Incident: count consecutive trailing \`up\` results. If count ≥ \`recovery_threshold\` → close Incident.
- \`degraded\` results are ignored — they do not count as \`up\` or \`down\` for threshold purposes.
- \`incidents\` DB table (\`id\`, \`monitor_id\`, \`opened_at\`, \`closed_at\` null = open)
- Evaluator triggers NotificationDispatcher (slice #10) on state change; for now, log the event if dispatcher not yet wired

## Acceptance criteria
- [ ] Incident opens exactly when failure_threshold consecutive \`down\` results are reached
- [ ] Incident closes exactly when recovery_threshold consecutive \`up\` results are reached after an open Incident
- [ ] \`degraded\` results do not affect threshold counting
- [ ] Only one open Incident per Monitor at any time
- [ ] Unit tests: pure in-memory CheckResult sequence assertions covering open, close, flap, degraded-ignored cases
- [ ] Integration test: full threshold crossing with real DB

## Blocked by
- #$I6" \
  "ready-for-agent")
echo "Created #$I8: IncidentEvaluator"

# ── Issue 9: Incident API ─────────────────────────────────────────────────────
I9=$(create_issue \
  "[Phase 1] #9 — Incident API + derived Monitor health" \
  "## Parent
#$PRD

## What to build
Expose Incidents and derived Monitor health via the REST API:
- \`GET /workspaces/:id/monitors/:mid/incidents\` — list Incidents; optional \`status=open|closed\` filter
- Monitor list and detail responses include derived \`health\` field: \`down\` if open Incident exists, \`up\` otherwise
- Health is derived on read via DB query (index on \`monitor_id WHERE closed_at IS NULL\`)

## Acceptance criteria
- [ ] Open Incidents appear in the list with \`closed_at: null\`
- [ ] Closed Incidents include both \`opened_at\` and \`closed_at\` (duration derivable)
- [ ] Monitor list response includes correct derived \`health\` field
- [ ] \`status=open\` filter returns only open Incidents
- [ ] Non-member receives 403
- [ ] Integration tests cover open, closed, and mixed Incident states

## Blocked by
- #$I4
- #$I8" \
  "ready-for-agent")
echo "Created #$I9: Incident API"

# ── Issue 10: NotificationDispatcher webhook (HITL) ───────────────────────────
I10=$(create_issue \
  "[Phase 1] #10 — NotificationDispatcher: webhook dispatch on Incident events (HITL)" \
  "## Parent
#$PRD

## What to build
When an Incident opens or closes, dispatch to all linked NotificationChannels and record each attempt:
- On Incident open/close: for each linked NotificationChannel, send HTTP POST to webhook URL with Incident payload
- Write \`Notification\` record: \`incident_id\`, \`notification_channel_id\`, \`event\` (\`opened\`/\`closed\`), \`delivery_status\`, \`sent_at\`
- \`notifications\` DB table
- Retry on transient failure (policy TBD — needs review)
- Wire into IncidentEvaluator so dispatch is triggered automatically

HITL: Retry policy, timeout, webhook payload schema, and what constitutes a delivery failure need review before merging.

## Acceptance criteria
- [ ] Webhook receives POST on Incident open with correct payload (monitor id, incident id, event, timestamp)
- [ ] Webhook receives POST on Incident close
- [ ] Notification record is written for each dispatch attempt with delivery status
- [ ] Failed delivery (non-2xx or timeout) is recorded; retry behavior reviewed and documented
- [ ] Webhook URL is decrypted at dispatch time — never logged
- [ ] Unit tests: mock HTTP server for webhook delivery + Notification record assertions
- [ ] Integration test: full open → dispatch → Notification record flow

## Blocked by
- #$I5
- #$I8" \
  "")
echo "Created #$I10: NotificationDispatcher"

# ── Issue 11: Notification API ────────────────────────────────────────────────
I11=$(create_issue \
  "[Phase 1] #11 — Notification API: delivery audit trail" \
  "## Parent
#$PRD

## What to build
Expose Notification delivery records via the REST API:
- \`GET /workspaces/:id/monitors/:mid/incidents/:iid/notifications\` — list Notifications for an Incident
- Returns: channel kind, event (opened/closed), delivery_status, sent_at

## Acceptance criteria
- [ ] Returns all Notification records for the specified Incident
- [ ] delivery_status reflects actual dispatch outcome (success/failed)
- [ ] Webhook URL is NOT returned — only channel id and kind
- [ ] Non-member receives 403
- [ ] Integration tests with pre-seeded Notifications

## Blocked by
- #$I9
- #$I10" \
  "ready-for-agent")
echo "Created #$I11: Notification API"

# ── Issue 12: Phase 1 E2E integration test ────────────────────────────────────
I12=$(create_issue \
  "[Phase 1] #12 — Phase 1 end-to-end integration test" \
  "## Parent
#$PRD

## What to build
A single automated end-to-end test that proves the full Phase 1 tracer bullet works:

Flow to validate:
1. Create Workspace + User (Owner)
2. Create uptime Monitor with failure_threshold=2, recovery_threshold=2
3. Create webhook NotificationChannel; attach to Monitor
4. Scheduler executes check → target returns 200 → CheckResult \`up\` stored
5. Target returns 503 twice → two \`down\` CheckResults → Incident opens
6. Webhook receives \`opened\` dispatch; Notification record written with \`success\`
7. Target returns 200 twice → two \`up\` CheckResults → Incident closes
8. Webhook receives \`closed\` dispatch; second Notification record written

Test uses a local mock HTTP server for both the monitored target and the webhook destination.

## Acceptance criteria
- [ ] Full flow completes without error in CI
- [ ] Incident opens after exactly failure_threshold consecutive \`down\` results
- [ ] Incident closes after exactly recovery_threshold consecutive \`up\` results
- [ ] Both Notification records (opened + closed) are written with \`delivery_status: success\`
- [ ] Webhook mock server receives correctly-shaped payloads for both events
- [ ] Test is deterministic and does not rely on real timers (use injectable clock or manual tick)

## Blocked by
- #$I11" \
  "ready-for-agent")
echo "Created #$I12: E2E test"

# ── Issue 13: Minimal web view (stretch) ─────────────────────────────────────
I13=$(create_issue \
  "[Phase 1] #13 — Minimal web view: Monitor list + derived health (stretch)" \
  "## Parent
#$PRD

## What to build
A read-only internal web view in \`signalnode-web\` showing Monitor status at a glance:
- List all Monitors in a Workspace with derived health (\`up\`/\`down\`)
- Click through to CheckResult history and open Incidents
- Auth via API token in config (no login UI required for Phase 1 internal use)

This is a stretch goal. Backend API must be complete first.

## Acceptance criteria
- [ ] Monitor list displays name, kind, interval, and derived health
- [ ] Clicking a Monitor shows recent CheckResults and any open Incident
- [ ] Page refreshes reflect current state
- [ ] No credentials appear in the frontend bundle or network responses

## Blocked by
- #$I9" \
  "ready-for-agent")
echo "Created #$I13: Web view"

# ── Issue 14: Email dispatch (follow-up, HITL) ────────────────────────────────
I14=$(create_issue \
  "[Follow-up] #14 — NotificationChannel: email dispatch (HITL)" \
  "## Parent
#$PRD

## What to build
Add email as a second NotificationChannel kind:
- \`kind: email\` in NotificationChannel config (SMTP address)
- SMTP client in NotificationDispatcher; credentials from env vars
- Notification records written for email dispatch same as webhook

This is a follow-up to Phase 1. Email adds SMTP/provider complexity deferred from the tracer bullet.

HITL: SMTP provider choice, credential handling, bounce/failure handling, and deliverability considerations need review.

## Acceptance criteria
- [ ] Email NotificationChannel can be created and attached to a Monitor
- [ ] Incident open/close triggers email to configured address
- [ ] SMTP credentials are not stored in DB; supplied via env vars only
- [ ] Notification record written for each email attempt with delivery status
- [ ] Unit tests: mock SMTP client for delivery assertions

## Blocked by
- #$I10" \
  "")
echo "Created #$I14: Email dispatch"

echo ""
echo "Done. Issues created:"
echo "  Bootstrap:              #$I1"
echo "  Auth:                   #$I2  (HITL)"
echo "  Workspace + RBAC:       #$I3  (HITL)"
echo "  Monitor CRUD:           #$I4"
echo "  NotificationChannel:    #$I5  (HITL)"
echo "  Scheduler + Executor:   #$I6"
echo "  CheckResult API:        #$I7"
echo "  IncidentEvaluator:      #$I8"
echo "  Incident API:           #$I9"
echo "  NotificationDispatcher: #$I10 (HITL)"
echo "  Notification API:       #$I11"
echo "  Phase 1 E2E test:       #$I12"
echo "  Web view (stretch):     #$I13"
echo "  Email dispatch:         #$I14 (HITL follow-up)"
