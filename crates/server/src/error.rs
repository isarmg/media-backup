use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use photo_backup_protocol::ErrorResponse;
use std::fmt;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
    retry_after: Option<u64>,
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn too_many_requests(retry_after: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "too many login attempts".to_owned(),
            retry_after: Some(retry_after.max(1)),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response();
        if let Some(retry_after) = self.retry_after {
            if let Ok(value) = retry_after.to_string().parse() {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        response
    }
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(?error, "database error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        tracing::error!(?error, "storage error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "storage error")
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::bad_request(format!("invalid JSON: {error}"))
    }
}
