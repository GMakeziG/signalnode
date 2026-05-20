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
