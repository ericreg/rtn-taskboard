use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct Error(pub StatusCode, pub &'static str, pub String);
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, "validation", message.into())
    }
    pub fn unauthorized() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Please sign in to continue.".into(),
        )
    }
    pub fn forbidden() -> Self {
        Self(
            StatusCode::FORBIDDEN,
            "forbidden",
            "You do not have permission to do that.".into(),
        )
    }
    pub fn missing() -> Self {
        Self(
            StatusCode::NOT_FOUND,
            "not_found",
            "This item could not be found.".into(),
        )
    }
    pub fn conflict() -> Self {
        Self(
            StatusCode::CONFLICT,
            "conflict",
            "This item changed. Reload it before saving your changes.".into(),
        )
    }
    pub fn full() -> Self {
        Self(StatusCode::INSUFFICIENT_STORAGE, "storage_full", "The database content limit has been reached. Remove content or ask an editor to raise the limit.".into())
    }
    pub fn image_too_large(max_bytes: u64) -> Self {
        Self(
            StatusCode::PAYLOAD_TOO_LARGE,
            "image_too_large",
            format!("Images may be at most {} MiB.", max_bytes / (1024 * 1024)),
        )
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":self.1,"message":self.2}})),
        )
            .into_response()
    }
}
impl From<turso::Error> for Error {
    fn from(err: turso::Error) -> Self {
        if let turso::Error::Constraint(ref message) = err {
            if message.contains("UNIQUE constraint") || message.contains("PRIMARY KEY constraint") {
                return Self(
                    StatusCode::CONFLICT,
                    "duplicate",
                    "That item already exists.".into(),
                );
            }
            if message.contains("FOREIGN KEY constraint") {
                return Self::bad("The referenced item is unavailable.");
            }
        }
        tracing::error!(error = %err, "Database operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "The operation could not be completed.".into(),
        )
    }
}
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        tracing::error!(error = %err, "I/O operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "The operation could not be completed.".into(),
        )
    }
}
