//! Error type shared by every facade function.

/// Coarse category of a facade error (400/404/409/500).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    BadRequest,
    NotFound,
    Conflict,
    Internal,
}

/// Typed error returned by every facade function. Carries the category plus
/// the exact message the HTTP route would have returned.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub message: String,
}

impl ApiError {
    /// 400 — invalid input, missing parameters, bad values.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { kind: ApiErrorKind::BadRequest, message: message.into() }
    }

    /// 404 — the requested resource does not exist.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self { kind: ApiErrorKind::NotFound, message: message.into() }
    }

    /// 409 — the request conflicts with current state (duplicate, already running, ...).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self { kind: ApiErrorKind::Conflict, message: message.into() }
    }

    /// 500 — internal failure (I/O, persistence, ...).
    pub fn internal(message: impl Into<String>) -> Self {
        Self { kind: ApiErrorKind::Internal, message: message.into() }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

/// Result alias used by every facade function.
pub type ApiResult<T> = Result<T, ApiError>;
