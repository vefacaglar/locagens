//! Code symbol search facade
//! (HTTP equivalent: `routes/symbols.rs`, Settings → Symbols / @-symbol mentions).

use crate::api::error::{ApiError, ApiResult};
use crate::AppState;
use crate::services::symbols::CodeSymbol;

/// Searches code symbols in a project directory: registered projects search
/// directly, unregistered-but-valid directories fall back to a canonicalized
/// search so the tab is usable before registration
/// (`GET /api/projects/symbols`).
pub async fn search_symbols(
    state: &AppState,
    path: &str,
    query: Option<&str>,
    kind: Option<&str>,
) -> ApiResult<Vec<CodeSymbol>> {
    let symbols = match crate::api::projects::require_registered_project(state, path) {
        Ok(canonical) => crate::services::symbols::search(
            &canonical,
            query.unwrap_or(""),
            kind,
            60,
        ),
        Err(_) => match crate::services::symbols::search_canonical(
            path,
            query.unwrap_or(""),
            kind,
        ) {
            Ok(symbols) => symbols,
            Err(err) => return Err(ApiError::bad_request(err)),
        },
    };
    Ok(symbols)
}
