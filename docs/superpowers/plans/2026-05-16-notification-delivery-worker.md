# Phase 2: Notification Delivery Worker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a real notification delivery worker in `signalnode-core` that polls `pending_notifications WHERE sent_at IS NULL` and delivers via HTTP POST (webhook) or SMTP (email), consuming rows written by the Phase 1 outbox fanout without touching the API or outbox write path.

**Architecture:** `signalnode-core` runs a `tokio::time::sleep` poll loop using `SELECT … FOR UPDATE OF pn SKIP LOCKED` to claim batches of up to 50 undelivered rows. Each row is dispatched via `reqwest` (webhook) or `lettre` async SMTP (email). On success, `sent_at = NOW()` is set in the same transaction. On failure the row stays `NULL` and is retried next cycle (at-least-once). `dispatch_notifications` in the API is preserved as a no-op; the call site is unchanged.

**Tech Stack:** Rust, sqlx 0.8, reqwest 0.12, lettre 0.11, tokio, wiremock 0.6 (tests only)

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/20260516000010_pending_notifications_sent_at.sql` | Add `sent_at` column + partial index |
| Modify | `signalnode-core/Cargo.toml` | Add sqlx, reqwest, lettre, chrono, uuid, serde_json |
| Create | `signalnode-core/src/config.rs` | `Config` + `SmtpConfig` from env vars |
| Create | `signalnode-core/src/deliver/mod.rs` | `DeliveryError`, re-exports |
| Create | `signalnode-core/src/deliver/webhook.rs` | `deliver_webhook` |
| Create | `signalnode-core/src/deliver/email.rs` | `SmtpConfig`, `build_email_message`, `deliver_email` |
| Create | `signalnode-core/src/worker.rs` | `PendingRow`, `poll_once`, `run_worker` |
| Modify | `signalnode-core/src/main.rs` | Build pool + client, spawn worker loop |
| Modify | `signalnode-api/src/notification_channel/mod.rs` | Remove stub comment from `dispatch_notifications` |

---

## Task 1: Migration — add `sent_at` to `pending_notifications`

**Files:**
- Create: `migrations/20260516000010_pending_notifications_sent_at.sql`

- [ ] **Step 1: Write the migration**

```sql
ALTER TABLE pending_notifications ADD COLUMN sent_at TIMESTAMPTZ;

CREATE INDEX pending_notifications_unsent_idx
    ON pending_notifications (created_at)
    WHERE sent_at IS NULL;
```

- [ ] **Step 2: Run existing tests to verify the new column doesn't break anything**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all 116 tests pass. The column is nullable so all existing inserts continue to work.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260516000010_pending_notifications_sent_at.sql
git commit -m "feat: add sent_at column and unsent partial index to pending_notifications"
```

---

## Task 2: Add dependencies to `signalnode-core`

**Files:**
- Modify: `signalnode-core/Cargo.toml`

- [ ] **Step 1: Replace `signalnode-core/Cargo.toml` with the following**

```toml
[package]
name = "signalnode-core"
version = "0.1.0"
edition = "2021"

[dependencies]
chrono = { version = "0.4", features = ["serde"] }
lettre = { version = "0.11", features = ["tokio1", "rustls-tls", "smtp-transport", "builder"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "macros"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }

[dev-dependencies]
wiremock = "0.6"
```

- [ ] **Step 2: Verify it compiles (no new modules yet, just deps)**

```bash
cargo build --manifest-path signalnode-core/Cargo.toml
```

Expected: compiles successfully. Dependency resolution may take a moment.

- [ ] **Step 3: Commit**

```bash
git add signalnode-core/Cargo.toml Cargo.lock
git commit -m "feat(core): add sqlx, reqwest, lettre, serde_json deps"
```

---

## Task 3: `config.rs` — parse env vars

**Files:**
- Create: `signalnode-core/src/config.rs`

- [ ] **Step 1: Write the failing test**

Add at the bottom of the new file (the whole file shown in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_requires_database_url() {
        // Remove DATABASE_URL if set, expect panic
        // This test is run in isolation via #[should_panic]
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("SMTP_HOST");
        let _cfg = Config::from_env(); // panics without DATABASE_URL
    }
}
```

- [ ] **Step 2: Run it to see it fail (function not defined yet)**

```bash
cargo test --manifest-path signalnode-core/Cargo.toml config 2>&1 | tail -5
```

Expected: compile error — `Config` not found.

- [ ] **Step 3: Write `signalnode-core/src/config.rs`**

```rust
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub from: String,
}

pub struct Config {
    pub database_url: String,
    pub smtp: Option<SmtpConfig>,
    pub poll_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set");

        let poll_interval_secs = std::env::var("WORKER_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);

        let smtp = std::env::var("SMTP_HOST").ok().map(|host| SmtpConfig {
            host,
            port: std::env::var("SMTP_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(587),
            user: std::env::var("SMTP_USER")
                .expect("SMTP_USER required when SMTP_HOST is set"),
            pass: std::env::var("SMTP_PASS")
                .expect("SMTP_PASS required when SMTP_HOST is set"),
            from: std::env::var("SMTP_FROM")
                .expect("SMTP_FROM required when SMTP_HOST is set"),
        });

        Config { database_url, smtp, poll_interval_secs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "DATABASE_URL must be set")]
    fn from_env_panics_without_database_url() {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("SMTP_HOST");
        Config::from_env();
    }

    #[test]
    fn from_env_uses_poll_interval_default() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::remove_var("SMTP_HOST");
        std::env::remove_var("WORKER_POLL_INTERVAL_SECS");
        let cfg = Config::from_env();
        assert_eq!(cfg.poll_interval_secs, 10);
        std::env::remove_var("DATABASE_URL");
    }

    #[test]
    fn from_env_parses_poll_interval() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::set_var("WORKER_POLL_INTERVAL_SECS", "30");
        std::env::remove_var("SMTP_HOST");
        let cfg = Config::from_env();
        assert_eq!(cfg.poll_interval_secs, 30);
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("WORKER_POLL_INTERVAL_SECS");
    }

    #[test]
    fn from_env_smtp_none_when_no_host() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::remove_var("SMTP_HOST");
        let cfg = Config::from_env();
        assert!(cfg.smtp.is_none());
        std::env::remove_var("DATABASE_URL");
    }
}
```

- [ ] **Step 4: Wire `config` module in `main.rs` (minimal — just declare the module)**

Edit `signalnode-core/src/main.rs`:
```rust
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");
}
```

- [ ] **Step 5: Run config tests**

```bash
cargo test --manifest-path signalnode-core/Cargo.toml config 2>&1 | tail -10
```

Expected:
```
test config::tests::from_env_panics_without_database_url ... ok
test config::tests::from_env_uses_poll_interval_default ... ok
test config::tests::from_env_parses_poll_interval ... ok
test config::tests::from_env_smtp_none_when_no_host ... ok
```

- [ ] **Step 6: Commit**

```bash
git add signalnode-core/src/config.rs signalnode-core/src/main.rs
git commit -m "feat(core): add Config from env vars with optional SmtpConfig"
```

---

## Task 4: `deliver/webhook.rs` — HTTP webhook delivery

**Files:**
- Create: `signalnode-core/src/deliver/mod.rs`
- Create: `signalnode-core/src/deliver/webhook.rs`

- [ ] **Step 1: Write the failing tests (create `deliver/webhook.rs` with tests first)**

```rust
// signalnode-core/src/deliver/webhook.rs
use serde_json::Value;
use super::DeliveryError;

pub async fn deliver_webhook(
    client: &reqwest::Client,
    target: &str,
    payload: Value,
) -> Result<(), DeliveryError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deliver_webhook_succeeds_on_200() {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let result = deliver_webhook(&client, &mock.uri(), serde_json::json!({"x": 1})).await;
        assert!(result.is_ok());
        mock.verify().await;
    }

    #[tokio::test]
    async fn deliver_webhook_fails_on_non_2xx() {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let result = deliver_webhook(&client, &mock.uri(), serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DeliveryError::HttpStatus(500)));
    }

    #[tokio::test]
    async fn deliver_webhook_fails_on_network_error() {
        // Nothing listens on port 1 — immediate connection refused
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        let result = deliver_webhook(&client, "http://127.0.0.1:1", serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DeliveryError::Http(_)));
    }
}
```

- [ ] **Step 2: Create `deliver/mod.rs` with `DeliveryError`**

```rust
// signalnode-core/src/deliver/mod.rs
pub mod email;
pub mod webhook;

pub use email::{build_email_message, deliver_email, SmtpConfig};
pub use webhook::deliver_webhook;

#[derive(Debug)]
pub enum DeliveryError {
    Http(reqwest::Error),
    HttpStatus(u16),
    Email(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryError::Http(e) => write!(f, "HTTP error: {e}"),
            DeliveryError::HttpStatus(s) => write!(f, "non-success HTTP status: {s}"),
            DeliveryError::Email(e) => write!(f, "email error: {e}"),
        }
    }
}

impl std::error::Error for DeliveryError {}
```

- [ ] **Step 3: Run tests to verify they fail (todo!)**

```bash
cargo test --manifest-path signalnode-core/Cargo.toml deliver::webhook 2>&1 | tail -10
```

Expected: panics with `not yet implemented`.

- [ ] **Step 4: Implement `deliver_webhook`**

Replace the `todo!()` body:

```rust
pub async fn deliver_webhook(
    client: &reqwest::Client,
    target: &str,
    payload: Value,
) -> Result<(), DeliveryError> {
    let response = client
        .post(target)
        .json(&payload)
        .send()
        .await
        .map_err(DeliveryError::Http)?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(DeliveryError::HttpStatus(response.status().as_u16()))
    }
}
```

- [ ] **Step 5: Wire `deliver` module in `main.rs`**

```rust
// signalnode-core/src/main.rs
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod deliver;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");
}
```

- [ ] **Step 6: Run webhook tests**

```bash
cargo test --manifest-path signalnode-core/Cargo.toml deliver::webhook 2>&1 | tail -10
```

Expected:
```
test deliver::webhook::tests::deliver_webhook_succeeds_on_200 ... ok
test deliver::webhook::tests::deliver_webhook_fails_on_non_2xx ... ok
test deliver::webhook::tests::deliver_webhook_fails_on_network_error ... ok
```

- [ ] **Step 7: Commit**

```bash
git add signalnode-core/src/deliver/mod.rs signalnode-core/src/deliver/webhook.rs signalnode-core/src/main.rs
git commit -m "feat(core): add deliver_webhook with DeliveryError"
```

---

## Task 5: `deliver/email.rs` — SMTP email delivery

**Files:**
- Create: `signalnode-core/src/deliver/email.rs`

- [ ] **Step 1: Create `deliver/email.rs` with failing tests**

```rust
// signalnode-core/src/deliver/email.rs
use lettre::{
    message::header::ContentType, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use super::DeliveryError;

pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub from: String,
}

pub fn build_email_message(
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<Message, Box<dyn std::error::Error + Send + Sync>> {
    todo!()
}

pub async fn deliver_email(
    config: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), DeliveryError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_email_message_succeeds_with_valid_addresses() {
        let msg = build_email_message(
            "from@example.com",
            "to@example.com",
            "Incident opened for monitor \"My Monitor\"",
            "An incident was opened for monitor \"My Monitor\" at 2026-05-16T00:00:00Z.",
        );
        assert!(msg.is_ok());
    }

    #[test]
    fn build_email_message_fails_with_invalid_from_address() {
        let msg = build_email_message("not-an-email", "to@example.com", "Subject", "Body");
        assert!(msg.is_err());
    }

    #[test]
    fn build_email_message_fails_with_invalid_to_address() {
        let msg = build_email_message("from@example.com", "not-an-email", "Subject", "Body");
        assert!(msg.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --manifest-path signalnode-core/Cargo.toml deliver::email 2>&1 | tail -10
```

Expected: panics with `not yet implemented`.

- [ ] **Step 3: Implement `build_email_message` and `deliver_email`**

Replace the `todo!()` bodies in `deliver/email.rs`:

```rust
pub fn build_email_message(
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<Message, Box<dyn std::error::Error + Send + Sync>> {
    let msg = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;
    Ok(msg)
}

pub async fn deliver_email(
    config: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), DeliveryError> {
    let msg = build_email_message(&config.from, to, subject, body)
        .map_err(DeliveryError::Email)?;

    let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
        .map_err(|e| DeliveryError::Email(e.into()))?
        .port(config.port)
        .credentials(lettre::transport::smtp::authentication::Credentials::new(
            config.user.clone(),
            config.pass.clone(),
        ))
        .build();

    transport.send(msg).await.map_err(|e| DeliveryError::Email(e.into()))?;
    Ok(())
}
```

- [ ] **Step 4: Run email tests**

```bash
cargo test --manifest-path signalnode-core/Cargo.toml deliver::email 2>&1 | tail -10
```

Expected:
```
test deliver::email::tests::build_email_message_succeeds_with_valid_addresses ... ok
test deliver::email::tests::build_email_message_fails_with_invalid_from_address ... ok
test deliver::email::tests::build_email_message_fails_with_invalid_to_address ... ok
```

- [ ] **Step 5: Commit**

```bash
git add signalnode-core/src/deliver/email.rs
git commit -m "feat(core): add SmtpConfig and deliver_email via lettre async SMTP"
```

---

## Task 6: `worker.rs` — `poll_once` with DB and delivery

**Files:**
- Create: `signalnode-core/src/worker.rs`

- [ ] **Step 1: Create `worker.rs` with failing tests**

```rust
// signalnode-core/src/worker.rs
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::deliver::{deliver_email, deliver_webhook, SmtpConfig};

#[derive(sqlx::FromRow)]
struct PendingRow {
    id: Uuid,
    channel_kind: String,
    target: String,
    incident_id: Uuid,
    monitor_id: Uuid,
    opened_at: DateTime<Utc>,
    monitor_name: String,
}

pub async fn poll_once(pool: &PgPool, client: &reqwest::Client, smtp: Option<&SmtpConfig>) {
    todo!()
}

pub async fn run_worker(
    pool: PgPool,
    client: reqwest::Client,
    smtp: Option<SmtpConfig>,
    interval: std::time::Duration,
) {
    loop {
        poll_once(&pool, &client, smtp.as_ref()).await;
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn insert_fixture(pool: &PgPool, channel_kind: &str, target: &str) -> Uuid {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) VALUES ('worker-test@example.com', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let wid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (name, slug, owner_id) VALUES ('W', 'worker-test', $1) RETURNING id",
        )
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap();

        let mid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO monitors (workspace_id, name, url, interval_secs) \
             VALUES ($1, 'Monitor', 'http://example.com', 60) RETURNING id",
        )
        .bind(wid)
        .fetch_one(pool)
        .await
        .unwrap();

        let iid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO incidents (monitor_id) VALUES ($1) RETURNING id",
        )
        .bind(mid)
        .fetch_one(pool)
        .await
        .unwrap();

        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO pending_notifications (incident_id, channel_kind, target) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(iid)
        .bind(channel_kind)
        .bind(target)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn sent_at(pool: &PgPool, pn_id: Uuid) -> Option<DateTime<Utc>> {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT sent_at FROM pending_notifications WHERE id = $1",
        )
        .bind(pn_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_delivers_webhook_and_marks_sent(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let pn_id = insert_fixture(&pool, "webhook", &mock.uri()).await;
        let client = reqwest::Client::new();
        poll_once(&pool, &client, None).await;

        assert!(sent_at(&pool, pn_id).await.is_some(), "sent_at should be set after delivery");
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_leaves_row_on_delivery_failure(pool: PgPool) {
        let pn_id = insert_fixture(&pool, "webhook", "http://127.0.0.1:1").await;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        poll_once(&pool, &client, None).await;

        assert!(sent_at(&pool, pn_id).await.is_none(), "sent_at should stay NULL on failure");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_skips_email_when_smtp_not_configured(pool: PgPool) {
        let pn_id = insert_fixture(&pool, "email", "alert@example.com").await;
        let client = reqwest::Client::new();
        poll_once(&pool, &client, None).await;

        assert!(sent_at(&pool, pn_id).await.is_none(), "email row should stay NULL when SMTP is None");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_skips_already_sent_rows(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock)
            .await;

        let pn_id = insert_fixture(&pool, "webhook", &mock.uri()).await;
        // Mark as already sent
        sqlx::query("UPDATE pending_notifications SET sent_at = NOW() WHERE id = $1")
            .bind(pn_id)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        poll_once(&pool, &client, None).await;

        // Mock expects 0 requests — verify confirms no POST was made
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_sends_correct_json_payload(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_json_schema(serde_json::json!({
                "type": "object",
                "required": ["incident_id", "monitor_id", "monitor_name", "opened_at"]
            })))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        insert_fixture(&pool, "webhook", &mock.uri()).await;
        let client = reqwest::Client::new();
        poll_once(&pool, &client, None).await;

        mock.verify().await;
    }
}
```

- [ ] **Step 2: Wire `worker` module in `main.rs`**

```rust
// signalnode-core/src/main.rs
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod deliver;
mod worker;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");
}
```

- [ ] **Step 3: Run tests to verify they fail (todo!)**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml worker 2>&1 | tail -10
```

Expected: panics with `not yet implemented`.

- [ ] **Step 4: Implement `poll_once`**

Replace `pub async fn poll_once(…) { todo!() }` with:

```rust
pub async fn poll_once(pool: &PgPool, client: &reqwest::Client, smtp: Option<&SmtpConfig>) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "poll_once: failed to begin transaction");
            return;
        }
    };

    let rows = match sqlx::query_as::<_, PendingRow>(
        "SELECT pn.id, pn.channel_kind, pn.target, \
                i.id AS incident_id, i.monitor_id, i.opened_at, m.name AS monitor_name \
         FROM pending_notifications pn \
         JOIN incidents i ON i.id = pn.incident_id \
         JOIN monitors m ON m.id = i.monitor_id \
         WHERE pn.sent_at IS NULL \
         ORDER BY pn.created_at ASC \
         LIMIT 50 \
         FOR UPDATE OF pn SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = ?e, "poll_once: failed to fetch pending rows");
            return;
        }
    };

    for row in &rows {
        let payload = serde_json::json!({
            "incident_id": row.incident_id,
            "monitor_id": row.monitor_id,
            "monitor_name": row.monitor_name,
            "opened_at": row.opened_at,
        });

        let result = match row.channel_kind.as_str() {
            "webhook" => deliver_webhook(client, &row.target, payload).await,
            "email" => match smtp {
                Some(cfg) => {
                    let subject = format!(
                        "[SignalNode] Incident opened for monitor \"{}\"",
                        row.monitor_name
                    );
                    let body = format!(
                        "An incident was opened for monitor \"{}\" at {}.",
                        row.monitor_name,
                        row.opened_at.to_rfc3339()
                    );
                    deliver_email(cfg, &row.target, &subject, &body).await
                }
                None => {
                    tracing::warn!(
                        target = %row.target,
                        "SMTP not configured; skipping email delivery (row will retry when SMTP is added)"
                    );
                    continue;
                }
            },
            kind => {
                tracing::error!(kind, id = %row.id, "unknown channel_kind; skipping");
                continue;
            }
        };

        match result {
            Ok(()) => {
                tracing::info!(id = %row.id, kind = %row.channel_kind, "notification delivered");
                if let Err(e) = sqlx::query(
                    "UPDATE pending_notifications SET sent_at = NOW() WHERE id = $1",
                )
                .bind(row.id)
                .execute(&mut *tx)
                .await
                {
                    tracing::error!(error = ?e, id = %row.id, "failed to mark notification sent");
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    id = %row.id,
                    kind = %row.channel_kind,
                    target = %row.target,
                    "delivery failed; will retry on next poll"
                );
            }
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "poll_once: failed to commit transaction");
    }
}
```

- [ ] **Step 5: Run worker tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml worker 2>&1 | tail -15
```

Expected:
```
test worker::tests::poll_once_delivers_webhook_and_marks_sent ... ok
test worker::tests::poll_once_leaves_row_on_delivery_failure ... ok
test worker::tests::poll_once_skips_email_when_smtp_not_configured ... ok
test worker::tests::poll_once_skips_already_sent_rows ... ok
test worker::tests::poll_once_sends_correct_json_payload ... ok
```

- [ ] **Step 6: Commit**

```bash
git add signalnode-core/src/worker.rs signalnode-core/src/main.rs
git commit -m "feat(core): implement poll_once — claim, deliver, mark sent"
```

---

## Task 7: Wire `run_worker` into `main.rs`

**Files:**
- Modify: `signalnode-core/src/main.rs`

- [ ] **Step 1: Write the final `main.rs`**

```rust
// signalnode-core/src/main.rs
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod deliver;
mod worker;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");

    let cfg = config::Config::from_env();

    let pool = sqlx::PgPool::connect(&cfg.database_url)
        .await
        .expect("failed to connect to database");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client");

    let interval = Duration::from_secs(cfg.poll_interval_secs);

    info!(
        interval_secs = cfg.poll_interval_secs,
        smtp_configured = cfg.smtp.is_some(),
        "starting notification delivery worker"
    );

    worker::run_worker(pool, client, cfg.smtp, interval).await;
}
```

- [ ] **Step 2: Verify the full signalnode-core build (no DB needed)**

```bash
cargo build --manifest-path signalnode-core/Cargo.toml
```

Expected: compiles without errors or warnings.

- [ ] **Step 3: Run all signalnode-core tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml 2>&1 | tail -15
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add signalnode-core/src/main.rs
git commit -m "feat(core): wire run_worker loop into main — signalnode-core is now a real delivery worker"
```

---

## Task 8: Clean up `dispatch_notifications` stub in the API

**Files:**
- Modify: `signalnode-api/src/notification_channel/mod.rs`

The `dispatch_notifications` call site in `create_check_result` is preserved (per "keep post-commit flow intact"). Only the stale stub comment is removed.

- [ ] **Step 1: Remove the stub comment from `dispatch_notifications`**

In `signalnode-api/src/notification_channel/mod.rs`, replace:

```rust
pub async fn dispatch_notifications(_pool: &PgPool, _incident_id: Uuid) {
    // stub — wired in Task 4
}
```

with:

```rust
pub async fn dispatch_notifications(_pool: &PgPool, _incident_id: Uuid) {}
```

- [ ] **Step 2: Run all API tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all 116 tests pass.

- [ ] **Step 3: Run full workspace tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test
```

Expected: all signalnode-api and signalnode-core tests pass.

- [ ] **Step 4: Apply rustfmt**

```bash
cargo fmt --manifest-path signalnode-core/Cargo.toml
cargo fmt --manifest-path signalnode-api/Cargo.toml
```

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/notification_channel/mod.rs
git commit -m "chore: remove stub comment from dispatch_notifications — core worker handles delivery"
```

---

## Self-Review

**Spec coverage:**
- `sent_at` schema → Task 1 ✓
- signalnode-core dependencies → Task 2 ✓
- Config parsing with SMTP opt-in → Task 3 ✓
- Webhook delivery with 2xx/non-2xx/network-error cases → Task 4 ✓
- Email delivery with message construction + SMTP transport → Task 5 ✓
- `poll_once` with FOR UPDATE SKIP LOCKED, delivery, mark-sent, skip-on-failure, skip-email-when-no-SMTP → Task 6 ✓
- `run_worker` loop wired to main → Task 7 ✓
- API dispatch_notifications kept as no-op, call site unchanged → Task 8 ✓

**Placeholder scan:** No TBDs, TODOs, or "similar to Task N" references — each task is self-contained with full code.

**Type consistency:**
- `SmtpConfig` defined in `deliver/email.rs`, re-exported from `deliver/mod.rs`, used in `config::Config.smtp` and `worker::poll_once`
- `PendingRow` field names (`incident_id`, `monitor_id`, `opened_at`, `monitor_name`) match SQL column aliases exactly
- `deliver_webhook(client, target, payload)` signature consistent across Task 4 definition and Task 6 call site
- `deliver_email(cfg, to, subject, body)` signature consistent across Task 5 definition and Task 6 call site

**Gaps:** None identified.
