//! Locagens backend binary (Axum + Tokio + file-based JSON/JSONL storage)
//! built on top of `pi`.
//!
//! Bootstraps the file-based data store, resolves the HTTP port
//! (settings.json → `PORT` env → 4321), binds to `127.0.0.1:{port}` and
//! prints `[locagens:backend-ready] port={port}` immediately after binding.
//! All shared logic lives in the `locagens_core` library (see `src/lib.rs`).

use axum::http::{header, HeaderValue, Method};
use axum::middleware;
use axum::Router;
use locagens_core::{build_core, routes, services, DEFAULT_PORT};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

fn resolve_port(settings_path: &std::path::Path) -> u16 {
    // settings.json wins, then PORT env, then the built-in default.
    if let Some(port) = routes::settings::read_stored_port(settings_path) {
        return port;
    }
    if let Ok(env_port) = std::env::var("PORT") {
        if let Ok(port) = env_port.trim().parse::<u16>() {
            if port >= 1 {
                return port;
            }
        }
    }
    DEFAULT_PORT
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let core = match build_core() {
        Ok(core) => core,
        Err(err) => {
            eprintln!("[locagens] {err}");
            std::process::exit(1);
        }
    };
    let settings_path = core.settings_path.clone();
    let port = resolve_port(&settings_path);
    let workspace_root = core.default_project_path.clone();

    core.db.run_startup_tasks(&workspace_root, "Locagens");

    let state = Arc::new(core.into_app_state(Arc::new(services::processes::ProcessManager::new())));

    let cors = CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://localhost:5173"),
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("null"),
        ])
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    let app = Router::new()
        .merge(routes::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            locagens_core::auth::auth_middleware,
        ))
        .layer(cors)
        .with_state(state);

    let address = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("[locagens] Failed to bind {address}: {err}");
            std::process::exit(1);
        }
    };

    // The desktop shell waits for this exact line before opening the UI.
    println!("[locagens:backend-ready] port={port}");

    if let Err(err) = axum::serve(listener, app).await {
        eprintln!("[locagens] Server error: {err}");
        std::process::exit(1);
    }
}
