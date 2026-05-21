use uuid::Uuid;

pub async fn evaluate_incident(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    monitor_id: Uuid,
    workspace_id: Uuid,
    failure_threshold: i32,
    recovery_threshold: i32,
) -> Result<Option<Uuid>, sqlx::Error> {
    let _ = (tx, monitor_id, workspace_id, failure_threshold, recovery_threshold);
    unimplemented!()
}
