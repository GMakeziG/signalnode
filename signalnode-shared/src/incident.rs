use uuid::Uuid;

pub async fn evaluate_incident(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    monitor_id: Uuid,
    workspace_id: Uuid,
    failure_threshold: i32,
    recovery_threshold: i32,
) -> Result<Option<Uuid>, sqlx::Error> {
    let open_incident = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1",
    )
    .bind(monitor_id)
    .fetch_optional(&mut **tx)
    .await?;

    if open_incident.is_none() {
        let recent = sqlx::query_scalar::<_, String>(
            "SELECT status FROM check_results \
             WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
        )
        .bind(monitor_id)
        .bind(failure_threshold)
        .fetch_all(&mut **tx)
        .await?;

        if recent.len() == failure_threshold as usize && recent.iter().all(|s| s == "down") {
            let incident_id = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO incidents (monitor_id) VALUES ($1) RETURNING id",
            )
            .bind(monitor_id)
            .fetch_one(&mut **tx)
            .await?;

            let channels = sqlx::query_as::<_, (String, String)>(
                "SELECT kind, target FROM notification_channels WHERE workspace_id = $1",
            )
            .bind(workspace_id)
            .fetch_all(&mut **tx)
            .await?;

            for (kind, target) in &channels {
                sqlx::query(
                    "INSERT INTO pending_notifications (incident_id, channel_kind, target) \
                     VALUES ($1, $2, $3)",
                )
                .bind(incident_id)
                .bind(kind)
                .bind(target)
                .execute(&mut **tx)
                .await?;
            }

            return Ok(Some(incident_id));
        }
    } else {
        let recent = sqlx::query_scalar::<_, String>(
            "SELECT status FROM check_results \
             WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
        )
        .bind(monitor_id)
        .bind(recovery_threshold)
        .fetch_all(&mut **tx)
        .await?;

        if recent.len() == recovery_threshold as usize && recent.iter().all(|s| s == "up") {
            sqlx::query(
                "UPDATE incidents SET closed_at = NOW() \
                 WHERE monitor_id = $1 AND closed_at IS NULL",
            )
            .bind(monitor_id)
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn setup(pool: &PgPool, failure_threshold: i32, recovery_threshold: i32) -> (Uuid, Uuid) {
        let uid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, password_hash) \
             VALUES ('shared-test@example.com', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let wid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (name, slug, owner_id) \
             VALUES ('W', 'shared-test', $1) RETURNING id",
        )
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap();

        let mid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO monitors (workspace_id, name, url, interval_secs, \
             failure_threshold, recovery_threshold) \
             VALUES ($1, 'M', 'http://example.com', 60, $2, $3) RETURNING id",
        )
        .bind(wid)
        .bind(failure_threshold)
        .bind(recovery_threshold)
        .fetch_one(pool)
        .await
        .unwrap();

        (wid, mid)
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn below_threshold_no_incident_opened(pool: PgPool) {
        let (wid, mid) = setup(&pool, 2, 1).await;
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'down')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 2, 1).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none(), "should return None when threshold not met");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM incidents WHERE monitor_id = $1")
                .bind(mid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn threshold_crossed_opens_incident_with_fanout(pool: PgPool) {
        let (wid, mid) = setup(&pool, 2, 1).await;
        sqlx::query(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'webhook', 'https://hooks.example.com/p11')",
        )
        .bind(wid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'down', NOW() - INTERVAL '10 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'down')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 2, 1).await.unwrap();
        tx.commit().await.unwrap();

        let incident_id = result.expect("should return Some(incident_id) when threshold crossed");
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1, "one open incident");
        let pn: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pending_notifications WHERE incident_id = $1")
                .bind(incident_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pn, 1, "one pending_notification per channel");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_second_incident_when_already_open(pool: PgPool) {
        let (wid, mid) = setup(&pool, 1, 2).await;
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'down')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 1, 2).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none(), "should return None when incident already open");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "still exactly one open incident");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn recovery_threshold_crossed_closes_incident(pool: PgPool) {
        let (wid, mid) = setup(&pool, 1, 2).await;
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'up', NOW() - INTERVAL '10 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'up')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 1, 2).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none(), "close path returns None");
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        let closed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NOT NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 0);
        assert_eq!(closed, 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn below_recovery_threshold_keeps_incident_open(pool: PgPool) {
        let (wid, mid) = setup(&pool, 1, 2).await;
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'up')")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let result = evaluate_incident(&mut tx, mid, wid, 1, 2).await.unwrap();
        tx.commit().await.unwrap();

        assert!(result.is_none());
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(mid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1, "incident should remain open");
    }
}
