# refresh_tokens Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a periodic Tokio task to signalnode-core that deletes expired `refresh_tokens` rows on a configurable interval (default 1 hour).

**Architecture:** New `purger.rs` module with `purge_once(pool)` and `run_purger(pool, interval)`, following the same pattern as `worker.rs` and `checker.rs`. Spawned as a third handle in `main.rs`. Interval driven by `TOKEN_PURGE_INTERVAL_SECS` env var added to `config.rs`.

**Tech Stack:** Rust, sqlx 0.8 (postgres), tokio 1, tracing 0.1

---

## File Map

| File | Change |
|---|---|
| `signalnode-core/src/config.rs` | Add `purge_interval_secs: u64` field + `TOKEN_PURGE_INTERVAL_SECS` parsing (default 3600) |
| `signalnode-core/src/purger.rs` | Create — `purge_once`, `run_purger`, integration tests |
| `signalnode-core/src/main.rs` | Add `mod purger;`, read `purge_interval` from config, spawn `h3`, join three handles |

---

## Task 1: Extend config.rs with TOKEN_PURGE_INTERVAL_SECS

**Files:**
- Modify: `signalnode-core/src/config.rs`

- [ ] **Step 1: Add `purge_interval_secs` field and parsing**

In `signalnode-core/src/config.rs`, add the field to the struct and parse it in `from_provider`. The final file should look like:

```rust
use crate::deliver::SmtpConfig;

pub struct Config {
    pub database_url: String,
    pub smtp: Option<SmtpConfig>,
    pub poll_interval_secs: u64,
    pub checker_poll_interval_secs: u64,
    pub purge_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        Self::from_provider(|k| std::env::var(k).ok())
    }

    fn from_provider(get: impl Fn(&str) -> Option<String>) -> Self {
        let database_url = get("DATABASE_URL").expect("DATABASE_URL must be set");

        let poll_interval_secs = get("WORKER_POLL_INTERVAL_SECS")
            .map(|v| v.parse::<u64>().expect("WORKER_POLL_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(10);

        let smtp = get("SMTP_HOST").map(|host| SmtpConfig {
            host,
            port: get("SMTP_PORT")
                .map(|v| v.parse::<u16>().expect("SMTP_PORT must be a valid port number"))
                .unwrap_or(587),
            user: get("SMTP_USER").expect("SMTP_USER required when SMTP_HOST is set"),
            pass: get("SMTP_PASS").expect("SMTP_PASS required when SMTP_HOST is set"),
            from: get("SMTP_FROM").expect("SMTP_FROM required when SMTP_HOST is set"),
        });

        let checker_poll_interval_secs = get("CHECKER_POLL_INTERVAL_SECS")
            .map(|v| v.parse::<u64>().expect("CHECKER_POLL_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(30);

        let purge_interval_secs = get("TOKEN_PURGE_INTERVAL_SECS")
            .map(|v| v.parse::<u64>().expect("TOKEN_PURGE_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(3600);

        Config { database_url, smtp, poll_interval_secs, checker_poll_interval_secs, purge_interval_secs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string())
    }

    #[test]
    #[should_panic(expected = "DATABASE_URL must be set")]
    fn from_env_panics_without_database_url() {
        Config::from_provider(|_| None);
    }

    #[test]
    fn from_env_uses_poll_interval_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.poll_interval_secs, 10);
    }

    #[test]
    fn from_env_parses_poll_interval() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("WORKER_POLL_INTERVAL_SECS", "30"),
        ]));
        assert_eq!(cfg.poll_interval_secs, 30);
    }

    #[test]
    fn from_env_smtp_none_when_no_host() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert!(cfg.smtp.is_none());
    }

    #[test]
    fn from_env_uses_checker_interval_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.checker_poll_interval_secs, 30);
    }

    #[test]
    fn from_env_parses_checker_interval() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("CHECKER_POLL_INTERVAL_SECS", "60"),
        ]));
        assert_eq!(cfg.checker_poll_interval_secs, 60);
    }

    #[test]
    fn from_env_uses_purge_interval_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.purge_interval_secs, 3600);
    }

    #[test]
    fn from_env_parses_purge_interval() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("TOKEN_PURGE_INTERVAL_SECS", "7200"),
        ]));
        assert_eq!(cfg.purge_interval_secs, 7200);
    }

    #[test]
    fn from_env_smtp_some_with_all_vars() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("SMTP_HOST", "smtp.example.com"),
            ("SMTP_PORT", "465"),
            ("SMTP_USER", "user@example.com"),
            ("SMTP_PASS", "secret"),
            ("SMTP_FROM", "from@example.com"),
        ]));
        let smtp = cfg.smtp.expect("smtp should be Some");
        assert_eq!(smtp.host, "smtp.example.com");
        assert_eq!(smtp.port, 465);
        assert_eq!(smtp.user, "user@example.com");
        assert_eq!(smtp.pass, "secret");
        assert_eq!(smtp.from, "from@example.com");
    }
}
```

- [ ] **Step 2: Run config tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml config
```

Expected output: all config tests pass, including the two new purge interval tests.

- [ ] **Step 3: Commit**

```bash
git add signalnode-core/src/config.rs
git commit -m "feat(core): add TOKEN_PURGE_INTERVAL_SECS to config (default 3600s)"
```

---

## Task 2: Write failing purger tests (red)

**Files:**
- Create: `signalnode-core/src/purger.rs`
- Modify: `signalnode-core/src/main.rs` (add `mod purger;` only)

- [ ] **Step 1: Create `purger.rs` with stub and failing tests**

Create `signalnode-core/src/purger.rs`:

```rust
use sqlx::PgPool;
use std::time::Duration;

pub async fn purge_once(pool: &PgPool) {
    // stub — intentionally empty so tests fail
}

pub async fn run_purger(pool: PgPool, interval: Duration) {
    loop {
        purge_once(&pool).await;
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use uuid::Uuid;

    #[sqlx::test(migrations = "../migrations")]
    async fn purge_once_deletes_expired_tokens(pool: PgPool) {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) VALUES ('purger-test@example.com', 'x') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let expired_jti = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO refresh_tokens (jti, user_id, expires_at) \
             VALUES ($1, $2, NOW() - INTERVAL '1 hour')",
        )
        .bind(expired_jti)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();

        let valid_jti = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO refresh_tokens (jti, user_id, expires_at) \
             VALUES ($1, $2, NOW() + INTERVAL '1 hour')",
        )
        .bind(valid_jti)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();

        purge_once(&pool).await;

        let expired_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM refresh_tokens WHERE jti = $1)",
        )
        .bind(expired_jti)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!expired_exists, "expired token should be deleted");

        let valid_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM refresh_tokens WHERE jti = $1)",
        )
        .bind(valid_jti)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(valid_exists, "valid token should survive purge");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn purge_once_no_op_when_nothing_expired(pool: PgPool) {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) VALUES ('purger-test2@example.com', 'x') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let valid_jti = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO refresh_tokens (jti, user_id, expires_at) \
             VALUES ($1, $2, NOW() + INTERVAL '1 hour')",
        )
        .bind(valid_jti)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();

        purge_once(&pool).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM refresh_tokens WHERE jti = $1",
        )
        .bind(valid_jti)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "valid token must not be deleted");
    }
}
```

- [ ] **Step 2: Add `mod purger;` to `main.rs`**

In `signalnode-core/src/main.rs`, add `mod purger;` alongside the other module declarations:

```rust
mod checker;
mod config;
mod deliver;
mod purger;
mod worker;
```

Do not change anything else in `main.rs` yet.

- [ ] **Step 3: Run purger tests to confirm red**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml purger
```

Expected: `purge_once_deletes_expired_tokens` **FAILS** (expired token still exists, stub is a no-op). `purge_once_no_op_when_nothing_expired` passes (stub no-op leaves the valid token alone — this test's assertion is trivially satisfied by the stub).

- [ ] **Step 4: Commit (red)**

```bash
git add signalnode-core/src/purger.rs signalnode-core/src/main.rs
git commit -m "test(core): add failing purger tests (red)"
```

---

## Task 3: Implement purge_once and wire main.rs (green)

**Files:**
- Modify: `signalnode-core/src/purger.rs` (replace stub body)
- Modify: `signalnode-core/src/main.rs` (read interval from config, spawn h3, join three handles)

- [ ] **Step 1: Replace stub with real `purge_once`**

In `signalnode-core/src/purger.rs`, replace the stub `purge_once` body:

```rust
use sqlx::PgPool;
use std::time::Duration;

pub async fn purge_once(pool: &PgPool) {
    match sqlx::query("DELETE FROM refresh_tokens WHERE expires_at < NOW()")
        .execute(pool)
        .await
    {
        Ok(r) => tracing::info!(rows_deleted = r.rows_affected(), "purged expired refresh tokens"),
        Err(e) => tracing::error!(error = ?e, "purge_once: failed to delete expired refresh tokens"),
    }
}

pub async fn run_purger(pool: PgPool, interval: Duration) {
    loop {
        purge_once(&pool).await;
        tokio::time::sleep(interval).await;
    }
}
```

Leave the `#[cfg(test)]` block unchanged.

- [ ] **Step 2: Run purger tests to confirm green**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml purger
```

Expected: both tests pass.

- [ ] **Step 3: Wire `main.rs`**

Replace the full contents of `signalnode-core/src/main.rs`:

```rust
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod checker;
mod config;
mod deliver;
mod purger;
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

    let worker_interval = Duration::from_secs(cfg.poll_interval_secs);
    let checker_interval = Duration::from_secs(cfg.checker_poll_interval_secs);
    let purge_interval = Duration::from_secs(cfg.purge_interval_secs);

    info!(
        worker_interval_secs = cfg.poll_interval_secs,
        checker_interval_secs = cfg.checker_poll_interval_secs,
        purge_interval_secs = cfg.purge_interval_secs,
        smtp_configured = cfg.smtp.is_some(),
        "signalnode-core starting workers"
    );

    let h1 = tokio::spawn(worker::run_worker(
        pool.clone(),
        client.clone(),
        cfg.smtp,
        worker_interval,
    ));
    let h2 = tokio::spawn(checker::run_checker(pool.clone(), client, checker_interval));
    let h3 = tokio::spawn(purger::run_purger(pool, purge_interval));
    let (r1, r2, r3) = tokio::join!(h1, h2, h3);
    r1.expect("delivery worker panicked");
    r2.expect("checker panicked");
    r3.expect("purger panicked");
}
```

Note: `pool.clone()` is now needed for `h2` since `h3` takes ownership of `pool`.

- [ ] **Step 4: Run the full signalnode-core test suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

Expected: **31 passed**, 0 failed (29 existing + 2 new purger tests).

- [ ] **Step 5: Run the full signalnode-api test suite to check for regressions**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: **131 passed**, 0 failed.

- [ ] **Step 6: Confirm total test count is 162**

Combined: 31 (core) + 131 (api) = **162**. If the count differs, do not proceed — investigate before committing.

- [ ] **Step 7: Commit (green)**

```bash
git add signalnode-core/src/purger.rs signalnode-core/src/main.rs
git commit -m "feat(core): implement refresh_tokens purger (purge_once + run_purger + main wiring)"
```

---

## Done

All 162 tests green. `refresh_tokens` expired rows are now purged hourly by default. `TOKEN_PURGE_INTERVAL_SECS` can be set to override the interval without recompiling.
