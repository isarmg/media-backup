use sqlx::PgPool;
use uuid::Uuid;

use crate::{auth::AuthContext, error::AppError};

pub async fn record(
    pool: &PgPool,
    auth: &AuthContext,
    action: &str,
    entity_kind: Option<&str>,
    entity_id: Option<Uuid>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events(account_id, actor_kind, actor_id, action, entity_kind, entity_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(auth.account_id)
    .bind(&auth.actor_kind)
    .bind(auth.actor_id)
    .bind(action)
    .bind(entity_kind)
    .bind(entity_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_change(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
    entity_kind: &str,
    entity_id: Uuid,
    operation: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO account_changes(account_id, entity_kind, entity_id, operation)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(account_id)
    .bind(entity_kind)
    .bind(entity_id)
    .bind(operation)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
