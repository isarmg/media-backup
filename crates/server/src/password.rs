use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::http::StatusCode;

use crate::error::AppError;

pub fn validate_password(password: &str) -> Result<(), AppError> {
    if password.len() < 8 || password.len() > 128 {
        return Err(AppError::bad_request(
            "password must contain 8 to 128 characters",
        ));
    }
    Ok(())
}

pub async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| {
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

pub async fn verify_password(password: String, encoded_hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&encoded_hash).ok().is_some_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password};

    #[tokio::test]
    async fn hashes_and_verifies_passwords() {
        let hash = hash_password("correct-horse-battery-staple".to_owned())
            .await
            .expect("password should hash");
        assert!(verify_password("correct-horse-battery-staple".to_owned(), hash.clone()).await);
        assert!(!verify_password("wrong-password".to_owned(), hash).await);
    }
}
