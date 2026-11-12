//! Code symbol search route (Settings → Symbols / @-symbol mentions).

use crate::util::json_error;
use crate::AppState;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SymbolsQuery {
    pub path: Option<String>,
    pub query: Option<String>,
    pub kind: Option<String>,
}

pub async fn search_symbols(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SymbolsQuery>,
) -> Response {
    let Some(raw_path) = query.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
        return json_error(
            axum::http::StatusCode::BAD_REQUEST,
            "Missing required query parameter: path",
        );
    };

    // Registered projects search directly; unregistered-but-valid directories
    // still work so the tab is usable before registration.
    let symbols = match crate::routes::projects::require_registered_project_pub(&state, raw_path) {
        Ok(canonical) => crate::services::symbols::search(
            &canonical,
            query.query.as_deref().unwrap_or(""),
            query.kind.as_deref(),
            60,
        ),
        Err(_) => match crate::services::symbols::search_canonical(
            raw_path,
            query.query.as_deref().unwrap_or(""),
            query.kind.as_deref(),
        ) {
            Ok(symbols) => symbols,
            Err(err) => return json_error(axum::http::StatusCode::BAD_REQUEST, &err),
        },
    };

    Json(json!({ "symbols": symbols })).into_response()
}
