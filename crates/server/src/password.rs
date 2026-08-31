use axum::http::StatusCode;

use crate::error::AppError;

pub fn require_current_policy(password: &str) -> Result<(), AppError> {
    sarmg_admin_auth::validate_password(password)
        .map_err(|error| AppError::bad_request(error.to_string()))
}

pub async fn hash_current_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        sarmg_admin_auth::hash_password(&password).map_err(|error| {
            tracing::error!(?error, "password hashing failed");
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "password hashing failed")
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(?error, "password hashing task failed");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "password hashing failed")
    })?
}

pub async fn verify_current_password(password: String, encoded_hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_current_password_blocking(&password, &encoded_hash))
        .await
        .unwrap_or(false)
}

pub(crate) fn verify_current_password_blocking(password: &str, encoded_hash: &str) -> bool {
    sarmg_admin_auth::verify_password(password, encoded_hash)
}

#[cfg(test)]
mod tests {
    use super::{hash_current_password, verify_current_password};

    #[tokio::test]
    async fn hashes_and_verifies_passwords() {
        let hash = hash_current_password("correct-horse-battery-staple".to_owned())
            .await
            .expect("password should hash");
        assert!(
            verify_current_password("correct-horse-battery-staple".to_owned(), hash.clone()).await
        );
        assert!(!verify_current_password("wrong-password".to_owned(), hash).await);
    }
}
