//! Code symbol search route (Settings → Symbols / @-symbol mentions). Thin
//! adapter over `crate::api::symbols`.

use crate::api::symbols;
use crate::routes::api_error_response;
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
        return api_error_response(crate::api::ApiError::bad_request(
            "Missing required query parameter: path",
        ));
    };

    match symbols::search_symbols(&state, raw_path, query.query.as_deref(), query.kind.as_deref())
        .await
    {
        Ok(list) => Json(json!({ "symbols": list })).into_response(),
        Err(err) => api_error_response(err),
    }
}
