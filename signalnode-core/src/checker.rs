use std::time::{Duration, Instant};

use futures::future::join_all;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct DueMonitor {
    id: Uuid,
    workspace_id: Uuid,
    url: String,
    failure_threshold: i32,
    recovery_threshold: i32,
    interval_secs: i32,
}

struct CheckOutcome {
    monitor: DueMonitor,
    status: &'static str,
    latency_ms: Option<i32>,
    error_detail: Option<String>,
}

pub async fn check_once(pool: &PgPool, _client: &reqwest::Client) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "check_once: failed to begin claim transaction");
            return;
        }
    };

    let monitors = match sqlx::query_as::<_, DueMonitor>(
        "SELECT id, workspace_id, url, failure_threshold, recovery_threshold, interval_secs \
         FROM monitors \
         WHERE status = 'active' \
           AND kind = 'uptime' \
           AND (last_checked_at IS NULL \
                OR last_checked_at + interval_secs * INTERVAL '1 second' <= NOW()) \
         ORDER BY last_checked_at ASC NULLS FIRST \
         LIMIT 50 \
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = ?e, "check_once: failed to fetch due monitors");
            return;
        }
    };

    if monitors.is_empty() {
        return;
    }

    for m in &monitors {
        if let Err(e) = sqlx::query("UPDATE monitors SET last_checked_at = NOW() WHERE id = $1")
            .bind(m.id)
            .execute(&mut *tx)
            .await
        {
            tracing::error!(error = ?e, monitor_id = %m.id, "check_once: failed to stamp last_checked_at");
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "check_once: failed to commit claim transaction");
    }
}

pub async fn run_checker(pool: PgPool, client: reqwest::Client, interval: Duration) {
    loop {
        check_once(&pool, &client).await;
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use sqlx::PgPool;

    async fn insert_monitor(pool: &PgPool, url: &str) -> (Uuid, Uuid) {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) \
             VALUES ('checker-test@example.com', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let wid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (name, slug, owner_id) \
             VALUES ('W', 'checker-test', $1) RETURNING id",
        )
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap();

        let mid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO monitors (workspace_id, name, url, interval_secs) \
             VALUES ($1, 'Monitor', $2, 60) RETURNING id",
        )
        .bind(wid)
        .bind(url)
        .fetch_one(pool)
        .await
        .unwrap();

        (wid, mid)
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_skips_paused_monitor(pool: PgPool) {
        let (_wid, mid) = insert_monitor(&pool, "http://127.0.0.1:1").await;
        sqlx::query("UPDATE monitors SET status = 'paused' WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let last_checked_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(last_checked_at.is_none(), "paused monitor should not have last_checked_at stamped");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_skips_not_yet_due_monitor(pool: PgPool) {
        let (_wid, mid) = insert_monitor(&pool, "http://127.0.0.1:1").await;
        // 30s ago with interval_secs=60 → not due for 30 more seconds
        sqlx::query(
            "UPDATE monitors SET last_checked_at = NOW() - INTERVAL '30 seconds' WHERE id = $1",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        let before: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let after: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(before, after, "not-yet-due monitor last_checked_at should be unchanged");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_updates_last_checked_at(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let last_checked_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT last_checked_at FROM monitors WHERE id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(last_checked_at.is_some(), "last_checked_at should be stamped after a check cycle");
    }
}
