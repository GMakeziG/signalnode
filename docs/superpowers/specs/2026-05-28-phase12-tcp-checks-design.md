# Phase 12: TCP Port Checks — Design Spec

**Date:** 2026-05-28
**Status:** Approved

## Goal

Add TCP port monitoring as a second check kind alongside the existing HTTP uptime checker. Scope is a narrow tracer bullet: no changes to incident evaluation, notification delivery, or the API's monitor PATCH kind-switching. DNS and SSL checks are future phases.

## Schema & Migration

File: `migrations/20260528000014_monitors_tcp_fields.sql`

```sql
ALTER TABLE monitors
    ALTER COLUMN url DROP NOT NULL,
    ADD COLUMN tcp_host TEXT,
    ADD COLUMN tcp_port INT
        CHECK (tcp_port IS NULL OR (tcp_port >= 1 AND tcp_port <= 65535)),
    ADD CONSTRAINT monitors_target_check CHECK (
        (kind = 'uptime' AND url IS NOT NULL)
        OR (kind = 'tcp' AND tcp_host IS NOT NULL AND tcp_port IS NOT NULL)
    );
```

**Rationale for each change:**

- `url DROP NOT NULL` — TCP monitors have no URL; `url` becomes `Option<String>` in Rust.
- `tcp_host TEXT` — hostname or IP for the TCP target.
- `tcp_port INT` — port number; DB-level range guard (1–65535). The existing `kind` column has no named CHECK constraint on its values, so no constraint needs to be replaced.
- `monitors_target_check` — enforces that every monitor has a valid target for its kind. No existing CHECK conflicts with this addition.

Existing `kind = 'uptime'` rows all have `url IS NOT NULL`, so the new constraint passes without a backfill.

## API Layer (signalnode-api)

### Structs

`Monitor` (response struct, `sqlx::FromRow`):
- `url: Option<String>` (was `String`)
- `+ tcp_host: Option<String>`
- `+ tcp_port: Option<i32>`

`CreateMonitorRequest`:
- `url: Option<String>` (was `String`)
- `+ kind: Option<String>` — `"uptime"` (default) or `"tcp"`
- `+ tcp_host: Option<String>`
- `+ tcp_port: Option<i32>`

`PatchMonitorRequest`: unchanged. Patching `kind`, `tcp_host`, or `tcp_port` is out of scope for this phase. Monitor kind is locked at creation.

### Validation in `create_monitor`

```
let kind = body.kind.as_deref().unwrap_or("uptime");
match kind {
    "uptime" => url must be Some and non-empty
    "tcp"    => tcp_host must be Some and non-empty
                tcp_port must be Some and in 1..=65535
    _        => InvalidInput("kind must be 'uptime' or 'tcp'")
}
```

### INSERT

For `kind = "uptime"`: bind `url = Some(...)`, `tcp_host = None`, `tcp_port = None`.
For `kind = "tcp"`: bind `url = None`, `tcp_host = Some(...)`, `tcp_port = Some(...)`.

Both fields are always written explicitly so the `monitors_target_check` constraint is satisfied without relying on defaults.

### Example: create a TCP monitor

```json
POST /api/workspaces/{workspace_id}/monitors
{
  "name": "Database port",
  "kind": "tcp",
  "tcp_host": "db.internal",
  "tcp_port": 5432,
  "interval_secs": 60
}
```

Response (201):
```json
{
  "id": "...",
  "workspace_id": "...",
  "name": "Database port",
  "kind": "tcp",
  "url": null,
  "tcp_host": "db.internal",
  "tcp_port": 5432,
  "interval_secs": 60,
  "status": "active",
  "failure_threshold": 1,
  "recovery_threshold": 1,
  "created_at": "...",
  "updated_at": "..."
}
```

## Checker (signalnode-core)

### Config

Add to `config.rs`:
```
tcp_check_timeout_ms: u64  // default 5000, env var TCP_CHECK_TIMEOUT_MS
```

### DueMonitor

Add fields: `kind: String`, `tcp_host: Option<String>`, `tcp_port: Option<i32>`.

### Claim query

```sql
WHERE status = 'active'
  AND kind IN ('uptime', 'tcp')
  AND (last_checked_at IS NULL
       OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW())
```

### check_once signature

```rust
pub async fn check_once(pool: &PgPool, client: &reqwest::Client, tcp_timeout: Duration)
```

`run_checker` gains the same parameter and threads it through from `main`.

### Concurrent check loop

```rust
match m.kind.as_str() {
    "uptime" => { /* existing reqwest GET logic, unchanged */ }
    "tcp"    => check_tcp(&host, port, tcp_timeout).await
    kind     => {
        tracing::error!(monitor_id = %m.id, kind, "check_once: unknown monitor kind");
        // isolated arm — add metric counter here during observability phase
        CheckOutcome { status: "down", latency_ms: None,
                       error_detail: Some(format!("unknown monitor kind: {kind}")), monitor: m }
    }
}
```

### check_tcp

```rust
async fn check_tcp(host: &str, port: u16, timeout_dur: Duration, monitor: DueMonitor) -> CheckOutcome {
    let addr = format!("{host}:{port}");
    let start = Instant::now();
    match tokio::time::timeout(timeout_dur, TcpStream::connect(&addr)).await {
        Ok(Ok(stream)) => {
            drop(stream); // close deterministically before returning
            CheckOutcome {
                latency_ms: Some(start.elapsed().as_millis() as i32),
                status: "up",
                error_detail: None,
                monitor,
            }
        }
        Ok(Err(e)) => CheckOutcome {
            latency_ms: None,
            status: "down",
            error_detail: Some(e.to_string()),
            monitor,
        },
        Err(_) => CheckOutcome {
            latency_ms: None,
            status: "down",
            error_detail: Some(format!("connection timed out after {}ms", timeout_dur.as_millis())),
            monitor,
        },
    }
}
```

The write + `evaluate_incident` phase is identical for both kinds — no branching needed there.

## Tests

### signalnode-core/src/checker.rs

- `check_once_tcp_up_when_port_open` — bind a `TcpListener` on `127.0.0.1:0`, insert a `kind='tcp'` monitor with the assigned port, run `check_once`, assert `status='up'` and `latency_ms IS NOT NULL`.
- `check_once_tcp_down_when_port_refused` — insert a `kind='tcp'` monitor pointing at `127.0.0.1:1`, run `check_once`, assert `status='down'`, `latency_ms IS NULL`, `error_detail IS NOT NULL`.

All existing HTTP tests remain green; the claim query change is additive.

### signalnode-api/src/monitor/mod.rs

- `create_tcp_monitor_success` — assert 201, `kind="tcp"`, `tcp_host`, `tcp_port` present, `url=null`.
- `create_tcp_monitor_missing_fields` — `kind=tcp` without `tcp_host` or `tcp_port` → 422 `invalid_input`.
- `create_tcp_monitor_invalid_port` — port 0 and port 65536 → 422 `invalid_input`.
- `create_tcp_monitor_rejects_empty_host` — `tcp_host=""` → 422 `invalid_input`.

## Out of Scope (Future Phases)

- **DNS checks** — Phase 13 or later; would add `kind='dns'` with resolver config.
- **SSL/TLS certificate expiry checks** — future phase; would add `kind='ssl'` with expiry threshold.
- **Per-monitor timeout** — no per-row timeout field exists yet; TCP timeout is a single process-level env var matching the existing HTTP client timeout pattern.
- **Patching monitor kind** — `PATCH /monitors/{id}` does not allow changing `kind`, `tcp_host`, or `tcp_port` in this phase.
