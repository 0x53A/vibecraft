//! Tentacles: multi-target MCP tool proxy, embedded in vibecraft server.
//!
//! Each session with tentacles enabled gets its own HTTP server (MCP + management API)
//! running on a tokio task, bound to a unique port.

pub mod docker;
pub mod host;
pub mod ssh;
pub mod targets;

use std::sync::Arc;

use axum::extract::{Json, Path, State as AxumState};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::Router;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use docker::DockerTarget;
use host::HostTarget;
use ssh::SshTarget;
use targets::{Target, TargetInfo, TargetRegistry};

/// Handle returned from `start()`. Drop or cancel to shut down the tentacles server.
pub struct TentaclesHandle {
    pub port: u16,
    pub registry: TargetRegistry,
    pub cancel: CancellationToken,
}

// ── Target parsing ──────────────────────────────────────────────────────────

/// Parse a JSON target config, creating containers from image/dockerfile if needed.
pub async fn parse_target_json(
    config: &serde_json::Value,
) -> anyhow::Result<(TargetInfo, Arc<Target>)> {
    let name = config["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Target missing 'name'"))?
        .to_string();

    // Validate name: used in container names, file paths, etc.
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        anyhow::bail!("Target name contains invalid characters (allowed: alphanumeric, _, -, .)");
    }
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("Target name must be 1-64 characters");
    }

    let target_type = config["targetType"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Target missing 'targetType'"))?;
    let params = &config["params"];

    match target_type {
        "host" => Ok((
            TargetInfo {
                name: name.clone(),
                target_type: "host".into(),
                params: serde_json::Value::Null,
            },
            Arc::new(Target::Host(HostTarget::new(name))),
        )),
        "container" => {
            let mode = params["mode"].as_str().unwrap_or("container");
            let volumes: Vec<String> = params["volumes"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let container_name = format!("tentacles-{name}-{}", uuid::Uuid::new_v4().to_string()[..8].to_owned());

            let target: Target = match mode {
                "container" => {
                    let container = params["container"]
                        .as_str()
                        .unwrap_or("tentacles")
                        .to_string();
                    Target::Docker(DockerTarget::new(name.clone(), container))
                }
                "image" => {
                    let image = params["image"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Container image target missing 'image'"))?;
                    Target::Docker(
                        DockerTarget::create_from_image(
                            name.clone(),
                            container_name.clone(),
                            image,
                            &volumes,
                        )
                        .await?,
                    )
                }
                "dockerfile" => {
                    let dockerfile = params["dockerfile"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Dockerfile target missing 'dockerfile'"))?;
                    Target::Docker(
                        DockerTarget::create_from_dockerfile(
                            name.clone(),
                            container_name.clone(),
                            dockerfile,
                            &volumes,
                        )
                        .await?,
                    )
                }
                other => anyhow::bail!("Unknown container mode: {other}"),
            };

            Ok((
                TargetInfo {
                    name: name.clone(),
                    target_type: "container".into(),
                    params: params.clone(),
                },
                Arc::new(target),
            ))
        }
        "ssh" => {
            let host_str = params["host"]
                .as_str()
                .unwrap_or("localhost")
                .to_string();
            Ok((
                TargetInfo {
                    name: name.clone(),
                    target_type: "ssh".into(),
                    params: serde_json::json!({ "host": host_str }),
                },
                Arc::new(Target::Ssh(SshTarget::new(name, host_str))),
            ))
        }
        other => anyhow::bail!("Unknown target type: {other}"),
    }
}

// ── Tool parameter types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct BashParams {
    /// Target to execute on (use list_targets tool to see available targets)
    target: String,
    /// The command to execute
    command: String,
    /// Optional timeout in milliseconds (max 600000)
    #[serde(default)]
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct ReadParams {
    target: String,
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct WriteParams {
    target: String,
    file_path: String,
    content: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct EditParams {
    target: String,
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct GlobParams {
    target: String,
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct GrepParams {
    target: String,
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(rename = "type", default)]
    file_type: Option<String>,
    #[serde(default = "default_output_mode")]
    output_mode: String,
    #[serde(rename = "-i", default)]
    case_insensitive: Option<bool>,
    #[serde(rename = "-C", default)]
    context: Option<usize>,
    #[serde(rename = "-A", default)]
    after: Option<usize>,
    #[serde(rename = "-B", default)]
    before: Option<usize>,
    #[serde(rename = "-n", default)]
    line_numbers: Option<bool>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    multiline: Option<bool>,
}

fn default_output_mode() -> String {
    "files_with_matches".to_string()
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct RebuildContainerParams {
    /// Target container to rebuild (must be a Dockerfile-based target with editable: true)
    target: String,
    /// New Dockerfile content. Overwrites the existing Dockerfile and rebuilds the container.
    /// The build uses an empty context, so COPY/ADD instructions will not have access to host files.
    dockerfile_content: String,
}

// ── MCP Server handler ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Tentacles {
    registry: TargetRegistry,
    tool_router: rmcp::handler::server::router::tool::ToolRouter<Self>,
}

impl Tentacles {
    fn new(registry: TargetRegistry) -> Self {
        Self {
            tool_router: Self::tool_router(),
            registry,
        }
    }

    async fn resolve_target(&self, name: &str) -> Result<Arc<Target>, String> {
        match self.registry.get(name).await {
            Some(t) => Ok(t),
            None => {
                let names = self.registry.names().await;
                Err(format!(
                    "Unknown target '{name}'. Available targets: {}",
                    if names.is_empty() { "(none)".to_string() } else { names.join(", ") }
                ))
            }
        }
    }
}

#[tool_router]
impl Tentacles {
    #[tool(name = "Bash", description = "Execute a bash command on the specified target")]
    async fn bash(&self, Parameters(params): Parameters<BashParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };
        let timeout_ms = params.timeout.unwrap_or(120_000).min(600_000);
        let timeout_secs = (timeout_ms as f64 / 1000.0).ceil() as u64;
        match target.exec(&["bash", "-c", &params.command], Some(timeout_secs)).await {
            Ok(o) => format_output(&o),
            Err(e) => format!("Exec failed on '{}': {e}", params.target),
        }
    }

    #[tool(name = "Read", description = "Read a file from the specified target with line numbers")]
    async fn read(&self, Parameters(params): Parameters<ReadParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };
        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(2000);
        let script = if offset > 0 {
            format!("cat -n {} | tail -n +{} | head -n {}", shell_escape(&params.file_path), offset, limit)
        } else {
            format!("cat -n {} | head -n {}", shell_escape(&params.file_path), limit)
        };
        match target.exec(&["bash", "-c", &script], Some(30)).await {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            Ok(o) => format!("Failed to read {}: {}", params.file_path, String::from_utf8_lossy(&o.stderr)),
            Err(e) => format!("Read failed: {e}"),
        }
    }

    #[tool(name = "Write", description = "Write content to a file on the specified target")]
    async fn write(&self, Parameters(params): Parameters<WriteParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };
        let dir = std::path::Path::new(&params.file_path).parent()
            .map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "/".to_string());
        if let Err(e) = target.exec(&["bash", "-c", &format!("mkdir -p {}", shell_escape(&dir))], Some(10)).await {
            return format!("mkdir failed: {e}");
        }
        match target.exec_with_stdin(&["bash", "-c", &format!("cat > {}", shell_escape(&params.file_path))], params.content.as_bytes(), Some(30)).await {
            Ok(_) => format!("Wrote {} bytes to {}", params.content.len(), params.file_path),
            Err(e) => format!("Write failed: {e}"),
        }
    }

    #[tool(name = "Edit", description = "Exact string replacement in a file on the specified target")]
    async fn edit(&self, Parameters(params): Parameters<EditParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };
        let output = match target.exec(&["cat", &params.file_path], Some(30)).await {
            Ok(o) => o, Err(e) => return format!("Failed to read file: {e}"),
        };
        if !output.status.success() {
            return format!("File not found: {}", String::from_utf8_lossy(&output.stderr));
        }
        let content = String::from_utf8_lossy(&output.stdout).to_string();
        let count = content.matches(&params.old_string).count();
        if count == 0 { return format!("old_string not found in {}", params.file_path); }
        if count > 1 && !params.replace_all { return format!("old_string appears {count} times. Use replace_all or provide more context."); }
        let new_content = if params.replace_all { content.replace(&params.old_string, &params.new_string) }
            else { content.replacen(&params.old_string, &params.new_string, 1) };
        match target.exec_with_stdin(&["bash", "-c", &format!("cat > {}", shell_escape(&params.file_path))], new_content.as_bytes(), Some(30)).await {
            Ok(_) => if params.replace_all { format!("Replaced {count} occurrences in {}", params.file_path) }
                     else { format!("Replaced 1 occurrence in {}", params.file_path) },
            Err(e) => format!("Failed to write: {e}"),
        }
    }

    #[tool(name = "Glob", description = "Find files matching a glob pattern on the specified target")]
    async fn glob(&self, Parameters(params): Parameters<GlobParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };
        let dir = params.path.as_deref().unwrap_or(".");
        let script = format!(
            "cd {} && find . -path './{}'  -not -path '*/.git/*' 2>/dev/null | sed 's|^\\./||' | while read -r f; do stat -c '%Y %n' \"$f\" 2>/dev/null; done | sort -rn | cut -d' ' -f2-",
            shell_escape(dir), shell_escape(&params.pattern),
        );
        match target.exec(&["bash", "-c", &script], Some(30)).await {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(e) => format!("Glob failed: {e}"),
        }
    }

    #[tool(name = "Grep", description = "Search file contents using regex on the specified target")]
    async fn grep(&self, Parameters(params): Parameters<GrepParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };
        let mut cmd = vec!["rg".to_string()];
        match params.output_mode.as_str() {
            "files_with_matches" => cmd.push("-l".into()),
            "count" => cmd.push("-c".into()),
            "content" => { if params.line_numbers.unwrap_or(true) { cmd.push("-n".into()); } }
            _ => cmd.push("-l".into()),
        }
        if params.case_insensitive.unwrap_or(false) { cmd.push("-i".into()); }
        if params.multiline.unwrap_or(false) { cmd.push("-U".into()); cmd.push("--multiline-dotall".into()); }
        if let Some(c) = params.context { cmd.push(format!("-C{c}")); }
        if let Some(a) = params.after { cmd.push(format!("-A{a}")); }
        if let Some(b) = params.before { cmd.push(format!("-B{b}")); }
        if let Some(ref g) = params.glob { cmd.push("--glob".into()); cmd.push(g.clone()); }
        if let Some(ref t) = params.file_type { cmd.push("--type".into()); cmd.push(t.clone()); }
        cmd.push("--".into());
        cmd.push(params.pattern.clone());
        if let Some(ref p) = params.path { cmd.push(p.clone()); }
        let rg_cmd = cmd.iter().map(|s| shell_escape(s)).collect::<Vec<_>>().join(" ");
        let script = if let Some(limit) = params.head_limit { format!("{rg_cmd} | head -n {limit}") } else { rg_cmd };
        match target.exec(&["bash", "-c", &script], Some(60)).await {
            Ok(o) => {
                let result = String::from_utf8_lossy(&o.stdout).to_string();
                if result.is_empty() && !o.status.success() {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    if stderr.is_empty() { "No matches found.".into() } else { format!("Grep error: {stderr}") }
                } else { result }
            }
            Err(e) => format!("Grep failed: {e}"),
        }
    }

    #[tool(name = "list_targets", description = "List available execution targets and their types")]
    async fn list_targets(&self) -> String {
        let targets = self.registry.list().await;
        if targets.is_empty() { return "No targets configured.".into(); }
        targets.iter().map(|t| {
            let editable = t.params.get("editable").and_then(|v| v.as_bool()).unwrap_or(false);
            if editable {
                format!("- {} ({}, editable)", t.name, t.target_type)
            } else {
                format!("- {} ({})", t.name, t.target_type)
            }
        }).collect::<Vec<_>>().join("\n")
    }

    #[tool(
        name = "RebuildContainer",
        description = "Overwrite the Dockerfile of an editable container target and rebuild the container. \
                        The build uses an empty context directory, so COPY/ADD instructions cannot access host files. \
                        Use this to install packages, change base images, or customize the environment."
    )]
    async fn rebuild_container(&self, Parameters(params): Parameters<RebuildContainerParams>) -> String {
        let target = match self.resolve_target(&params.target).await {
            Ok(t) => t, Err(e) => return e,
        };

        // Check editable flag
        let info = match self.registry.get_info(&params.target).await {
            Some(i) => i,
            None => return format!("Target '{}' not found", params.target),
        };
        let editable = info.params.get("editable").and_then(|v| v.as_bool()).unwrap_or(false);
        if !editable {
            return format!(
                "Target '{}' is not editable. Create it with editable: true to allow Dockerfile modifications.",
                params.target
            );
        }

        // Must be a Docker target with a Dockerfile
        let dt = match target.as_ref() {
            Target::Docker(dt) => dt,
            _ => return format!("Target '{}' is not a container target", params.target),
        };

        let dockerfile_path = match &dt.dockerfile {
            Some(p) => p.clone(),
            None => return format!("Target '{}' was not created from a Dockerfile", params.target),
        };

        // Write new Dockerfile content
        if let Err(e) = tokio::fs::write(&dockerfile_path, &params.dockerfile_content).await {
            return format!("Failed to write Dockerfile at {dockerfile_path}: {e}");
        }

        // Rebuild
        match dt.rebuild().await {
            Ok((new_dt, build_log)) => {
                // Replace target in registry
                let mut new_info = info.clone();
                // Update params with new container info if needed
                if let Some(obj) = new_info.params.as_object_mut() {
                    obj.insert("container".into(), serde_json::Value::String(new_dt.container.clone()));
                }
                self.registry.add(new_info, Arc::new(Target::Docker(new_dt))).await;
                format!("Container rebuilt successfully.\n\n{build_log}")
            }
            Err(e) => format!("Rebuild failed: {e}"),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Tentacles {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().build();
        let mut info = ServerInfo::new(capabilities);
        info.server_info.name = "tentacles".into();
        info.server_info.version = "0.2.0".into();
        info.instructions = Some(
            "Multi-target tool execution. Every tool takes a 'target' parameter. Use list_targets to see available targets. \
             Targets marked 'editable' support the RebuildContainer tool for customizing the environment.".into()
        );
        info
    }
}

// ── Management API ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    registry: TargetRegistry,
}

async fn api_list_targets(AxumState(state): AxumState<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "targets": state.registry.list().await }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddTargetRequest {
    name: String,
    target_type: String,
    #[serde(default)]
    params: serde_json::Value,
}

async fn api_add_target(
    AxumState(state): AxumState<AppState>,
    Json(req): Json<AddTargetRequest>,
) -> impl IntoResponse {
    let config = serde_json::json!({
        "name": req.name,
        "targetType": req.target_type,
        "params": req.params,
    });
    match parse_target_json(&config).await {
        Ok((info, target)) => {
            state.registry.add(info, target).await;
            (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "error": e.to_string() }))).into_response(),
    }
}

async fn api_remove_target(
    AxumState(state): AxumState<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if state.registry.remove(&name).await {
        Json(serde_json::json!({ "ok": true }))
    } else {
        Json(serde_json::json!({ "ok": false, "error": "Target not found" }))
    }
}

async fn api_health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true }))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn format_output(output: &std::process::Output) -> String {
    let mut result = String::new();
    if !output.stdout.is_empty() { result.push_str(&String::from_utf8_lossy(&output.stdout)); }
    if !output.stderr.is_empty() {
        if !result.is_empty() { result.push('\n'); }
        result.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        result.push_str(&format!("\n\nExit code: {}", output.status.code().unwrap_or(-1)));
    }
    result
}

// ── Public API: start a tentacles server for a session ──────────────────────

/// Start a tentacles MCP + management HTTP server.
/// If `preferred_port` is given, tries to bind to that port first (for reconnecting
/// to an existing Claude session after server restart). Falls back to a random port.
pub async fn start(targets_json: &[crate::types::TentaclesTarget], preferred_port: Option<u16>) -> anyhow::Result<TentaclesHandle> {
    let registry = TargetRegistry::new();

    // Parse and add targets
    for target_cfg in targets_json {
        let config = serde_json::to_value(target_cfg)?;
        match parse_target_json(&config).await {
            Ok((info, target)) => {
                tracing::info!("Tentacles: adding target {} ({})", info.name, info.target_type);
                registry.add(info, target).await;
            }
            Err(e) => {
                tracing::warn!("Tentacles: failed to add target {}: {e}", target_cfg.name);
            }
        }
    }

    // Verify existing containers
    for info in registry.list().await {
        if info.target_type == "container" {
            let mode = info.params.get("mode").and_then(|v| v.as_str()).unwrap_or("container");
            if mode == "container" {
                if let Some(container) = info.params.get("container").and_then(|v| v.as_str()) {
                    let dt = DockerTarget::new(info.name.clone(), container.to_string());
                    if let Err(e) = dt.ensure_running().await {
                        tracing::warn!("Container target '{}' not ready: {e}", info.name);
                    }
                }
            }
        }
    }

    let ct = CancellationToken::new();

    // MCP service
    let mcp_registry = registry.clone();
    let mcp_config = rmcp::transport::streamable_http_server::StreamableHttpServerConfig {
        stateful_mode: true,
        cancellation_token: ct.clone(),
        ..Default::default()
    };
    let session_manager = rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default();
    let mcp_service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || Ok(Tentacles::new(mcp_registry.clone())),
        Arc::new(session_manager),
        mcp_config,
    );

    // Management API + MCP on same port
    let app = Router::new()
        .route("/targets", get(api_list_targets).post(api_add_target))
        .route("/targets/{name}", delete(api_remove_target))
        .route("/health", get(api_health))
        .with_state(AppState { registry: registry.clone() })
        .nest_service("/mcp", mcp_service);

    let listener = if let Some(p) = preferred_port {
        match tokio::net::TcpListener::bind(format!("127.0.0.1:{p}")).await {
            Ok(l) => {
                tracing::info!("Tentacles: rebound to preferred port {p}");
                l
            }
            Err(e) => {
                tracing::warn!("Tentacles: preferred port {p} unavailable ({e}), using random");
                tokio::net::TcpListener::bind("127.0.0.1:0").await?
            }
        }
    } else {
        tokio::net::TcpListener::bind("127.0.0.1:0").await?
    };
    let port = listener.local_addr()?.port();

    let shutdown_ct = ct.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown_ct.cancelled().await })
            .await
            .ok();
    });

    tracing::info!("Tentacles server started on port {port}");

    Ok(TentaclesHandle { port, registry, cancel: ct })
}
