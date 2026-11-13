//! `lcgn mcp list|login|logout`.

use super::args::McpAction;
use crate::api::mcp as mcp_api;
use crate::services::mcp::oauth::open_browser;
use crate::state::AppState;

pub async fn mcp_command(state: &AppState, action: McpAction, project: Option<&str>) -> i32 {
    match action {
        McpAction::List => match mcp_api::list_mcp_servers(state, project).await {
            Ok(response) if response.servers.is_empty() => {
                println!("No MCP servers configured.");
                0
            }
            Ok(response) => {
                for server in response.servers {
                    let auth = if server.needs_login {
                        " (needs login)"
                    } else if server.signed_in {
                        " (signed in)"
                    } else {
                        ""
                    };
                    println!(
                        "{}  {}  {}{auth}  {} tools",
                        server.config.name,
                        server.config.transport,
                        server.status,
                        server.tools.len()
                    );
                }
                0
            }
            Err(err) => fail(err),
        },
        McpAction::Login { server } => {
            let on_url = |url: &str| {
                eprintln!("Open this URL to sign in to '{server}':\n{url}");
                if open_browser(url).is_err() {
                    eprintln!("(Could not open a browser automatically; open the URL manually.)");
                }
            };
            match mcp_api::login_mcp_server(state, &server, project, &on_url).await {
                Ok(info) => {
                    println!("Signed in to '{}' ({}, {} tools).", info.config.name, info.status, info.tools.len());
                    0
                }
                Err(err) => fail(err),
            }
        }
        McpAction::Logout { server } => match mcp_api::logout_mcp_server(state, &server, project).await {
            Ok(true) => {
                println!("Signed out of '{server}'.");
                0
            }
            Ok(false) => {
                println!("No stored sign-in for '{server}'.");
                0
            }
            Err(err) => fail(err),
        },
    }
}

fn fail(err: crate::api::error::ApiError) -> i32 {
    eprintln!("lcgn: {err}");
    1
}
