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
                        "SMTP not configured; skipping email row (will retry when SMTP is added)"
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

    async fn get_sent_at(pool: &PgPool, pn_id: Uuid) -> Option<DateTime<Utc>> {
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

        assert!(get_sent_at(&pool, pn_id).await.is_some(), "sent_at should be set after delivery");
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

        assert!(get_sent_at(&pool, pn_id).await.is_none(), "sent_at should stay NULL on failure");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_skips_email_when_smtp_not_configured(pool: PgPool) {
        let pn_id = insert_fixture(&pool, "email", "alert@example.com").await;
        let client = reqwest::Client::new();
        poll_once(&pool, &client, None).await;

        assert!(get_sent_at(&pool, pn_id).await.is_none(), "email row should stay NULL when SMTP is None");
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
        sqlx::query("UPDATE pending_notifications SET sent_at = NOW() WHERE id = $1")
            .bind(pn_id)
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        poll_once(&pool, &client, None).await;

        mock.verify().await;
    }

    struct JsonHasKeys(&'static [&'static str]);

    impl wiremock::Match for JsonHasKeys {
        fn matches(&self, request: &wiremock::Request) -> bool {
            if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&request.body) {
                if let Some(obj) = body.as_object() {
                    return self.0.iter().all(|k| obj.contains_key(*k));
                }
            }
            false
        }
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn poll_once_sends_correct_json_payload(pool: PgPool) {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(JsonHasKeys(&["incident_id", "monitor_id", "monitor_name", "opened_at"]))
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
