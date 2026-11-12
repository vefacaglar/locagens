//! HTTP route modules. All handlers speak camelCase JSON to match
//! `packages/shared/src/index.ts` and the legacy Fastify routes.

pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod plugins;
pub mod processes;
pub mod projects;
pub mod providers;
pub mod runs;
pub mod security;
pub mod settings;
pub mod skills;
pub mod symbols;

use crate::AppState;
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::sync::Arc;

/// All routes, mounted on the root router (paths include the `/api` prefix).
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Health
        .route("/ping", get(ping))
        // Settings
        .route("/api/settings", get(settings::get_settings).put(settings::put_settings))
        // Providers (pi auth.json / models.json / models-store.json)
        .route("/api/providers", get(providers::list_providers))
        .route("/api/providers/config", get(providers::get_config).post(providers::save_config))
        .route("/api/providers/fetch-models", post(providers::fetch_models))
        .route("/api/providers/test", post(providers::test_provider))
        // Agent presets
        .route("/api/agent-presets", get(providers::get_presets).put(providers::put_presets))
        // Native pi auth endpoints
        .route("/api/pi/auth", get(providers::get_pi_auth).post(providers::set_pi_auth))
        // Projects & files & git
        .route("/api/projects", get(projects::list_projects).post(projects::create_project).delete(projects::delete_project))
        .route("/api/projects/files", get(projects::list_files))
        .route("/api/projects/select-dir", post(projects::select_dir))
        .route("/api/projects/git/status", get(projects::git_status))
        .route("/api/projects/git/diff-details", get(projects::git_diff_details))
        .route("/api/projects/git/generate-message", post(projects::git_generate_message))
        .route("/api/projects/git/commit", post(projects::git_commit))
        .route("/api/projects/symbols", get(symbols::search_symbols))
        // Memories
        .route("/api/memories", get(memory::list_memories).post(memory::create_memory).delete(memory::clear_memories))
        .route("/api/memories/:id", put(memory::update_memory).delete(memory::delete_memory))
        // Permissions
        .route("/api/permissions", get(permissions::list_permissions).delete(permissions::clear_permissions))
        .route("/api/permissions/:id", delete(permissions::delete_permission))
        // Skills
        .route("/api/skills", get(skills::list_skills))
        .route("/api/skills/install", post(skills::install_skill))
        .route("/api/skills/open-folder", post(skills::open_folder))
        .route("/api/skills/:name", get(skills::get_skill).delete(skills::delete_skill))
        // Processes
        .route("/api/processes", get(processes::list_processes))
        .route("/api/processes/spawn", post(processes::spawn_process))
        .route("/api/processes/:id/kill", post(processes::kill_process))
        .route("/api/processes/:id/restart", post(processes::restart_process))
        .route("/api/processes/:id/logs", get(processes::get_logs))
        // Plugins
        .route("/api/plugins", get(plugins::list_plugins))
        .route("/api/plugins/templates", get(plugins::list_templates))
        .route("/api/plugins/install", post(plugins::install_plugin))
        .route("/api/plugins/:id/toggle", post(plugins::toggle_plugin))
        .route("/api/plugins/:id", delete(plugins::delete_plugin))
        // MCP
        .route("/api/mcp/servers", get(mcp::list_servers).post(mcp::upsert_server))
        .route("/api/mcp/servers/:name", delete(mcp::delete_server))
        .route("/api/mcp/servers/:name/restart", post(mcp::restart_server))
        .route("/api/mcp/servers/:name/toggle", post(mcp::toggle_server))
        // Security
        .route("/api/security/status", get(security::status))
        .route("/api/security/sandbox/setup", post(security::sandbox_setup))
        // Runs (agent orchestrator + SSE)
        .route("/api/runs", get(runs::list_runs).post(runs::create_run))
        .route("/api/runs/:id", get(runs::get_run))
        .route("/api/runs/:id/continue", post(runs::continue_run))
        .route("/api/runs/:id/cancel", post(runs::cancel_run))
        .route("/api/runs/:id/messages", post(runs::queue_message))
        .route("/api/runs/:id/permission", post(runs::permission_decision))
        .route("/api/runs/:id/answer", post(runs::answer_question))
        .route("/api/runs/:id/messages", get(runs::list_messages))
        .route("/api/runs/:id/usage", get(runs::run_usage))
        .route("/api/runs/:id/plan", get(runs::run_plan))
        .route("/api/runs/:id/pending", get(runs::run_pending))
        .route("/api/runs/:id/events", get(runs::run_events))
        // Usage logs
        .route("/api/usage-logs", get(runs::usage_logs))
}

pub async fn ping() -> axum::response::Response {
    axum::Json(serde_json::json!({
        "status": "ok",
        "timestamp": crate::util::now_iso()
    }))
    .into_response()
}

use axum::response::IntoResponse;
