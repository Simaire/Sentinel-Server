//! Erreurs renvoyées aux clients : `{ "code": "...", "message": "..." }`.
//! Les détails internes sont journalisés, jamais envoyés.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("Authentification requise.")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("Compte en attente de validation par un administrateur.")]
    Pending,
    #[error("Introuvable.")]
    NotFound,
    #[error("Trop de requêtes, réessaie dans un instant.")]
    TooManyRequests,
    #[error("Erreur interne du serveur.")]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Pending => (StatusCode::FORBIDDEN, "pending"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Self::Internal(e) = &self {
            tracing::error!("erreur interne : {e:#}");
        }
        let (status, code) = self.parts();
        (status, Json(json!({ "code": code, "message": self.to_string() }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
