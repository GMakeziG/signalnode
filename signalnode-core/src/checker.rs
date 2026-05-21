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

pub async fn check_once(pool: &PgPool, client: &reqwest::Client) {
    // Phase 1: claim due monitors in a short transaction, stamp last_checked_at immediately
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
        return;
    }

    // HTTP checks — no DB locks held; all monitors checked concurrently
    let outcomes: Vec<CheckOutcome> = join_all(monitors.into_iter().map(|m| {
        let client = client.clone();
        async move {
            let start = Instant::now();
            match client.get(&m.url).send().await {
                Ok(resp) if resp.status().is_success() => CheckOutcome {
                    latency_ms: Some(start.elapsed().as_millis() as i32),
                    status: "up",
                    error_detail: None,
                    monitor: m,
                },
                Ok(resp) => CheckOutcome {
                    latency_ms: Some(start.elapsed().as_millis() as i32),
                    status: "down",
                    error_detail: Some(format!("HTTP {}", resp.status().as_u16())),
                    monitor: m,
                },
                Err(e) => CheckOutcome {
                    latency_ms: None,
                    status: "down",
                    error_detail: Some(e.to_string()),
                    monitor: m,
                },
            }
        }
    }))
    .await;

    // Phase 2: write results — one transaction per monitor for error isolation
    for outcome in outcomes {
        let mut tx = match pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to begin write transaction");
                continue;
            }
        };

        if let Err(e) = sqlx::query(
            "INSERT INTO check_results (monitor_id, status, latency_ms, error_detail) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(outcome.monitor.id)
        .bind(outcome.status)
        .bind(outcome.latency_ms)
        .bind(outcome.error_detail.as_deref())
        .execute(&mut *tx)
        .await
        {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to insert check_result");
            continue;
        }

        if let Err(e) = signalnode_shared::incident::evaluate_incident(
            &mut tx,
            outcome.monitor.id,
            outcome.monitor.workspace_id,
            outcome.monitor.failure_threshold,
            outcome.monitor.recovery_threshold,
        )
        .await
        {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: incident evaluation failed");
        }

        if let Err(e) = tx.commit().await {
            tracing::error!(error = ?e, monitor_id = %outcome.monitor.id, "check_once: failed to commit write transaction");
        } else {
            tracing::info!(monitor_id = %outcome.monitor.id, status = outcome.status, "check result written");
        }
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

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_writes_check_result_for_due_monitor(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "up");
        assert!(row.1.is_some(), "latency_ms should be recorded for 200 response");
        assert!(row.2.is_none(), "error_detail should be None for up");
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_marks_down_on_non_2xx(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "down");
        assert!(row.1.is_some(), "latency_ms should be recorded even for non-2xx");
        assert_eq!(row.2.as_deref(), Some("HTTP 500"));
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_marks_down_on_connect_error(pool: PgPool) {
        let (_wid, mid) = insert_monitor(&pool, "http://127.0.0.1:1").await;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        check_once(&pool, &client).await;

        let row: (String, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT status, latency_ms, error_detail FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "down");
        assert!(row.1.is_none(), "latency_ms should be None when no response received");
        assert!(row.2.is_some(), "error_detail should contain the error message");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_concurrent_no_duplicate(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let (_wid, _mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();
        tokio::join!(check_once(&pool, &client), check_once(&pool, &client));

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM check_results")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "FOR UPDATE SKIP LOCKED must prevent duplicate check_results");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_opens_incident_on_threshold(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock)
            .await;

        let (wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        sqlx::query("UPDATE monitors SET failure_threshold = 1 WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'webhook', 'https://hooks.example.com/test')",
        )
        .bind(wid)
        .execute(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let incident_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(incident_count, 1, "incident should open when failure_threshold is crossed");

        let pn_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_notifications pn \
             JOIN incidents i ON i.id = pn.incident_id \
             WHERE i.monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pn_count, 1, "one pending_notification per notification_channel");
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_closes_incident_on_recovery(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        sqlx::query("UPDATE monitors SET recovery_threshold = 1 WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let open_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_count, 0, "incident should be closed after recovery");

        let closed_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NOT NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(closed_count, 1);
        mock.verify().await;
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn check_once_no_duplicate_incident(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        sqlx::query("UPDATE monitors SET failure_threshold = 1 WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        check_once(&pool, &client).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "must not open a second incident when one is already open");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn run_checker_loops(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let (_wid, mid) = insert_monitor(&pool, &mock.uri()).await;
        let client = reqwest::Client::new();

        // Tick 1
        check_once(&pool, &client).await;

        // Simulate the monitor's interval having elapsed by backdating last_checked_at
        sqlx::query(
            "UPDATE monitors \
             SET last_checked_at = last_checked_at - INTERVAL '2 minutes' \
             WHERE id = $1",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        // Tick 2
        check_once(&pool, &client).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM check_results WHERE monitor_id = $1",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2, "two check_once ticks should produce two check_results");
    }
}
