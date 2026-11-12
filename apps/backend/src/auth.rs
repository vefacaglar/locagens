//! Bearer-token authentication for `/api/*` routes.
//!
//! When `LOCAGENS_API_TOKEN` is set (non-empty), every `/api/*` request must
//! carry `Authorization: Bearer <token>`; the comparison is constant time via
//! `subtle`. When unset, local development requests pass straight through.

use crate::util::json_error;
use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::AppState;

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Still run a comparison to keep timing flat, then fail.
        let _ = a.ct_eq(b);
        return false;
    }
    a.ct_eq(b).into()
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if !req.uri().path().starts_with("/api/") {
        return next.run(req).await;
    }

    if let Some(expected) = state.auth_token.as_ref() {
        let provided = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_string);

        let ok = provided
            .map(|provided| constant_time_eq(provided.as_bytes(), expected.as_bytes()))
            .unwrap_or(false);

        if !ok {
            return json_error(StatusCode::UNAUTHORIZED, "Unauthorized");
        }
    }

    next.run(req).await
}
