use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::PgPool;
use std::borrow::Cow;
use uuid::Uuid;

use crate::ErrorBody;

pub enum AuthzError {
    Forbidden,
    NotFound,
    Internal,
}

impl IntoResponse for AuthzError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AuthzError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            AuthzError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            AuthzError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}

pub async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzError> {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => {
            match sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
            )
            .bind(workspace_id)
            .fetch_one(pool)
            .await
            {
                Ok(true) => Err(AuthzError::Forbidden),
                Ok(false) => Err(AuthzError::NotFound),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(AuthzError::Internal)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(AuthzError::Internal)
        }
    }
}

pub async fn check_owner(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzError> {
    match sqlx::query_scalar::<_, String>(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(role)) if role == "owner" => Ok(()),
        Ok(Some(_)) => Err(AuthzError::Forbidden),
        Ok(None) => match sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
        )
        .bind(workspace_id)
        .fetch_one(pool)
        .await
        {
            Ok(true) => Err(AuthzError::Forbidden),
            Ok(false) => Err(AuthzError::NotFound),
            Err(e) => {
                tracing::error!(error = ?e, "failed to check workspace existence");
                Err(AuthzError::Internal)
            }
        },
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace owner");
            Err(AuthzError::Internal)
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{check_membership, check_owner, AuthzError};
    use crate::test_helpers::{create_test_user, create_test_workspace};

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_ok_for_member(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        assert!(check_membership(&pool, wid, uid).await.is_ok());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_forbidden_for_non_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let outsider = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        let result = check_membership(&pool, wid, outsider).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn membership_not_found_for_missing_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4();
        let result = check_membership(&pool, wid, uid).await;
        assert!(matches!(result, Err(AuthzError::NotFound)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_ok_for_owner(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        assert!(check_owner(&pool, wid, uid).await.is_ok());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_forbidden_for_plain_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let member = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(wid)
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
        let result = check_owner(&pool, wid, member).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_forbidden_for_non_member(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let outsider = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        let result = check_owner(&pool, wid, outsider).await;
        assert!(matches!(result, Err(AuthzError::Forbidden)));
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn owner_not_found_for_missing_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4();
        let result = check_owner(&pool, wid, uid).await;
        assert!(matches!(result, Err(AuthzError::NotFound)));
    }
}
