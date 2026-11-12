//! Locagens backend (Axum + Tokio + Rusqlite).
//!
//! Bootstraps the SQLite database, resolves the HTTP port (settings.json →
//! `PORT` env → 4321), binds to `127.0.0.1:{port}` and prints
//! `[locagens:backend-ready] port={port}` immediately after binding.

mod auth;
mod db;
mod paths;
mod routes;
mod services;
mod util;

use axum::http::{header, HeaderValue, Method};
use axum::middleware;
use axum::Router;
use services::processes::ProcessManager;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

pub const DEFAULT_PORT: u16 = 4321;

/// Shared application state passed to every handler.
pub struct AppState {
    pub db: db::Db,
    pub settings_path: PathBuf,
    pub default_project_path: PathBuf,
    pub processes: Arc<ProcessManager>,
    pub auth_token: Option<String>,
}

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

    let settings_path = paths::default_settings_path();
    let port = resolve_port(&settings_path);
    let db_path = db::resolve_db_path();

    let database = match db::Db::open(&db_path) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("[locagens] Failed to open database at {}: {err}", db_path.display());
            std::process::exit(1);
        }
    };

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    database.run_startup_tasks(&workspace_root, "Locagens");

    let default_project_path = workspace_root.clone();
    let state = Arc::new(AppState {
        db: database,
        settings_path,
        default_project_path,
        processes: Arc::new(ProcessManager::new()),
        auth_token: std::env::var("LOCAGENS_API_TOKEN")
            .ok()
            .filter(|token| !token.is_empty()),
    });

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
            auth::auth_middleware,
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
