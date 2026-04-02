use std::time::Duration;

use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Json as JsonResponse};
use axum::routing::{delete, get, post, put};
use axum::Router;
use ractor_wormhole::util::ActorRef_Ask;
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{debug, info};

use crate::actors::events::EventsMsg;
use crate::actors::sessions_supervisor::SessionsMsg;
use crate::actors::Actors;
use crate::hook::RawHookEvent;
use crate::types::{
    ClaudeEvent, CreateSessionRequest, CreateTextTileRequest, LinkSessionRequest, PromptRequest,
    SessionPromptRequest, UpdateSessionRequest, UpdateTextTileRequest,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const ASK_TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

// ── Router builder ──────────────────────────────────────────────────────────

pub fn router(actors: Actors) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(
            |origin: &HeaderValue, _| {
                let Ok(s) = origin.to_str() else {
                    return false;
                };
                let Ok(url) = url::Url::parse(s) else {
                    return false;
                };
                match url.host_str() {
                    Some("localhost") | Some("127.0.0.1") => true,
                    //Some("vibecraft.sh") if url.scheme() == "https" => true,
                    _ => false,
                }
            },
        ))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    Router::new()
        .route("/hook", post(post_hook))
        .route("/event", post(post_event))
        .route("/health", get(get_health))
        .route("/config", get(get_config))
        .route("/stats", get(get_stats))
        .route("/info", get(get_info))
        .route(
            "/prompt",
            get(get_prompt).post(post_prompt).delete(delete_prompt),
        )
        .route("/tmux-output", get(get_tmux_output))
        .route("/cancel", post(post_cancel))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/resumable", get(list_resumable_sessions))
        .route("/sessions/refresh", post(refresh_sessions))
        .route(
            "/sessions/{id}",
            get(get_session).patch(update_session).delete(delete_session),
        )
        .route("/sessions/{id}/prompt", post(session_prompt))
        .route("/sessions/{id}/cancel", post(session_cancel))
        .route("/sessions/{id}/permission", post(session_permission))
        .route("/sessions/{id}/restart", post(session_restart))
        .route("/sessions/{id}/link", post(session_link))
        .route("/sessions/{id}/targets", get(list_targets).post(add_target))
        .route("/sessions/{id}/targets/{name}", delete(remove_target))
        .route("/projects", get(list_projects))
        .route("/projects/autocomplete", get(autocomplete_projects))
        .route("/projects/{path}", delete(delete_project))
        .route("/docker/containers", get(list_docker_containers))
        .route("/docker/images", get(list_docker_images))
        .route("/tiles", get(list_tiles).post(create_tile))
        .route("/tiles/{id}", put(update_tile).delete(delete_tile))
        .route("/ws", get(crate::websocket::handle_ws_upgrade))
        .fallback_service(ServeDir::new("dist").fallback(ServeDir::new("dist")))
        .layer(cors)
        .with_state(actors)
}

// ── Event ingestion ─────────────────────────────────────────────────────────

async fn post_event(
    State(actors): State<Actors>,
    Json(event): Json<ClaudeEvent>,
) -> impl IntoResponse {
    debug!("Received event via HTTP: {}", event.event_type());
    let _ = actors.events.cast(EventsMsg::Ingest(event));
    (
        StatusCode::OK,
        JsonResponse(serde_json::json!({ "ok": true })),
    )
}

// ── Raw hook ingestion ──────────────────────────────────────────────────────

async fn post_hook(
    State(actors): State<Actors>,
    headers: axum::http::HeaderMap,
    Json(raw): Json<RawHookEvent>,
) -> impl IntoResponse {
    let session_id = raw.session_id.clone();
    let tmux_session = headers
        .get("x-tmux-session")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    debug!(
        "Raw hook: {} (session={}, tmux={:?}, transcript={:?})",
        raw.hook_event_name,
        raw.session_id,
        tmux_session.as_deref().unwrap_or("none"),
        raw.transcript_path.as_deref().unwrap_or("none"),
    );

    // Convert raw hook event → internal ClaudeEvent (includes transcript reading)
    let event = match raw.into_claude_event().await {
        Some(e) => e,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                JsonResponse(serde_json::json!({ "ok": false, "error": "Unknown hook event" })),
            );
        }
    };

    // Append to events.jsonl for persistence
    if let Ok(line) = serde_json::to_string(&event) {
        let events_file = &actors.config.events_file;
        if let Some(parent) = events_file.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        use tokio::io::AsyncWriteExt;
        if let Ok(mut f) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_file)
            .await
        {
            let _ = f.write_all(format!("{line}\n").as_bytes()).await;
        }
    }

    // Ingest into event processor (broadcast + dedup + stats)
    let _ = actors.events.cast(EventsMsg::Ingest(event));

    // Auto-link: match the claude session ID to a managed session via tmux name.
    // The hook sends X-Tmux-Session header with the tmux session name.
    if let Some(tmux_name) = tmux_session {
        let _ = actors
            .sessions
            .cast(SessionsMsg::LinkByTmux(tmux_name, session_id));
    }

    (
        StatusCode::OK,
        JsonResponse(serde_json::json!({ "ok": true })),
    )
}

// ── Health / Config / Stats ─────────────────────────────────────────────────

async fn get_health(State(actors): State<Actors>) -> impl IntoResponse {
    let event_count = actors
        .events
        .ask(EventsMsg::GetEventCount, ASK_TIMEOUT)
        .await
        .unwrap_or(0);
    let client_count = actors.broadcast_tx.receiver_count();

    JsonResponse(serde_json::json!({
        "ok": true,
        "version": VERSION,
        "clients": client_count,
        "events": event_count,
        "voiceEnabled": false,
    }))
}

async fn get_config(State(actors): State<Actors>) -> impl IntoResponse {
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "claude-user".into());
    let host = gethostname();
    JsonResponse(serde_json::json!({
        "username": username,
        "hostname": host,
        "tmuxSession": actors.config.tmux_session,
    }))
}

async fn get_stats(State(actors): State<Actors>) -> impl IntoResponse {
    let stats = actors
        .events
        .ask(EventsMsg::GetStats, ASK_TIMEOUT)
        .await;

    match stats {
        Ok(stats) => JsonResponse(serde_json::json!({
            "totalEvents": stats.total_events,
            "toolCounts": stats.tool_counts,
            "avgDurations": stats.avg_durations,
            "tokens": {},
        })),
        Err(_) => JsonResponse(serde_json::json!({
            "totalEvents": 0,
            "toolCounts": {},
            "avgDurations": {},
            "tokens": {},
        })),
    }
}

async fn get_info() -> impl IntoResponse {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    JsonResponse(serde_json::json!({ "ok": true, "cwd": cwd }))
}

// ── Prompt management ───────────────────────────────────────────────────────

async fn post_prompt(
    State(actors): State<Actors>,
    Json(req): Json<PromptRequest>,
) -> impl IntoResponse {
    if req.prompt.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            JsonResponse(serde_json::json!({ "error": "Prompt is required" })),
        );
    }

    if let Some(parent) = actors.config.pending_prompt_file.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    if let Err(e) = tokio::fs::write(&actors.config.pending_prompt_file, &req.prompt).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "error": format!("Failed to save prompt: {e}") })),
        );
    }

    info!(
        "Prompt saved: {}...",
        &req.prompt[..req.prompt.len().min(50)]
    );

    if req.send.unwrap_or(false) {
        match crate::tmux::send_to_tmux_safe(
            &actors.config.tmux_session,
            &req.prompt,
            &actors.config.exec_path,
        )
        .await
        {
            Ok(()) => {
                return (
                    StatusCode::OK,
                    JsonResponse(serde_json::json!({
                        "ok": true,
                        "saved": actors.config.pending_prompt_file.display().to_string(),
                        "sent": true,
                    })),
                );
            }
            Err(e) => {
                return (
                    StatusCode::OK,
                    JsonResponse(serde_json::json!({
                        "ok": true,
                        "saved": actors.config.pending_prompt_file.display().to_string(),
                        "sent": false,
                        "tmuxError": e.to_string(),
                    })),
                );
            }
        }
    }

    (
        StatusCode::OK,
        JsonResponse(serde_json::json!({
            "ok": true,
            "saved": actors.config.pending_prompt_file.display().to_string(),
        })),
    )
}

async fn get_prompt(State(actors): State<Actors>) -> impl IntoResponse {
    match tokio::fs::read_to_string(&actors.config.pending_prompt_file).await {
        Ok(prompt) => JsonResponse(serde_json::json!({
            "prompt": prompt,
            "file": actors.config.pending_prompt_file.display().to_string(),
        })),
        Err(_) => JsonResponse(serde_json::json!({ "prompt": null })),
    }
}

async fn delete_prompt(State(actors): State<Actors>) -> impl IntoResponse {
    let _ = tokio::fs::remove_file(&actors.config.pending_prompt_file).await;
    info!("Pending prompt cleared");
    JsonResponse(serde_json::json!({ "ok": true }))
}

// ── Legacy tmux output / cancel ─────────────────────────────────────────────

async fn get_tmux_output(State(actors): State<Actors>) -> impl IntoResponse {
    if !is_valid_tmux_session(&actors.config.tmux_session) {
        return JsonResponse(serde_json::json!({
            "ok": false,
            "error": "Invalid tmux session name",
            "output": "",
        }));
    }

    match crate::tmux::exec_with_path(
        "tmux",
        &[
            "capture-pane",
            "-t",
            &actors.config.tmux_session,
            "-p",
            "-S",
            "-100",
        ],
        &actors.config.exec_path,
    )
    .await
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            JsonResponse(serde_json::json!({ "ok": true, "output": text }))
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr);
            JsonResponse(serde_json::json!({ "ok": false, "error": err.to_string(), "output": "" }))
        }
        Err(e) => {
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string(), "output": "" }))
        }
    }
}

async fn post_cancel(State(actors): State<Actors>) -> impl IntoResponse {
    if !is_valid_tmux_session(&actors.config.tmux_session) {
        return (
            StatusCode::BAD_REQUEST,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Invalid tmux session name" })),
        );
    }

    match crate::tmux::exec_with_path(
        "tmux",
        &["send-keys", "-t", &actors.config.tmux_session, "C-c"],
        &actors.config.exec_path,
    )
    .await
    {
        Ok(output) if output.status.success() => {
            info!(
                "Sent Ctrl+C to tmux session: {}",
                actors.config.tmux_session
            );
            (
                StatusCode::OK,
                JsonResponse(serde_json::json!({ "ok": true })),
            )
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr);
            (
                StatusCode::OK,
                JsonResponse(serde_json::json!({ "ok": false, "error": err.to_string() })),
            )
        }
        Err(e) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

// ── Sessions ────────────────────────────────────────────────────────────────

async fn list_sessions(State(actors): State<Actors>) -> impl IntoResponse {
    let sessions = actors
        .sessions
        .ask(SessionsMsg::List, ASK_TIMEOUT)
        .await
        .unwrap_or_default();
    JsonResponse(serde_json::json!({ "ok": true, "sessions": sessions }))
}

async fn list_resumable_sessions() -> impl IntoResponse {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let sessions_dir = std::path::PathBuf::from(home).join(".claude").join("sessions");

    let mut resumable: Vec<serde_json::Value> = Vec::new();

    if let Ok(mut entries) = tokio::fs::read_dir(&sessions_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(contents) = tokio::fs::read_to_string(&path).await {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&contents) {
                    if parsed.get("sessionId").is_some()
                        && parsed.get("cwd").is_some()
                        && parsed.get("startedAt").is_some()
                    {
                        resumable.push(parsed);
                    }
                }
            }
        }
    }

    // Sort by startedAt descending (most recent first)
    resumable.sort_by(|a, b| {
        let a_ts = a.get("startedAt").and_then(|v| v.as_u64()).unwrap_or(0);
        let b_ts = b.get("startedAt").and_then(|v| v.as_u64()).unwrap_or(0);
        b_ts.cmp(&a_ts)
    });

    // Limit to 20
    resumable.truncate(20);

    JsonResponse(serde_json::json!({ "ok": true, "sessions": resumable }))
}

async fn create_session(
    State(actors): State<Actors>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let result = actors
        .sessions
        .ask(|reply| SessionsMsg::Create(req, reply), ASK_TIMEOUT)
        .await;

    match result {
        Ok(Ok(session)) => (
            StatusCode::CREATED,
            JsonResponse(serde_json::json!({ "ok": true, "session": session })),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

async fn refresh_sessions(State(actors): State<Actors>) -> impl IntoResponse {
    info!("Manual session refresh requested");
    let sessions = actors
        .sessions
        .ask(SessionsMsg::Refresh, ASK_TIMEOUT)
        .await
        .unwrap_or_default();
    JsonResponse(serde_json::json!({ "ok": true, "sessions": sessions }))
}

async fn get_session(
    State(actors): State<Actors>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let session = actors
        .sessions
        .ask(|reply| SessionsMsg::Get(id, reply), ASK_TIMEOUT)
        .await
        .ok()
        .flatten();

    match session {
        Some(session) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "session": session })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Session not found" })),
        ),
    }
}

async fn update_session(
    State(actors): State<Actors>,
    Path(id): Path<String>,
    Json(updates): Json<UpdateSessionRequest>,
) -> impl IntoResponse {
    let session = actors
        .sessions
        .ask(
            |reply| SessionsMsg::Update(id, updates, reply),
            ASK_TIMEOUT,
        )
        .await
        .ok()
        .flatten();

    match session {
        Some(session) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "session": session })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Session not found" })),
        ),
    }
}

async fn delete_session(
    State(actors): State<Actors>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let found = actors
        .sessions
        .ask(|reply| SessionsMsg::Delete(id, reply), ASK_TIMEOUT)
        .await
        .unwrap_or(false);

    if found {
        (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Session not found" })),
        )
    }
}

async fn session_prompt(
    State(actors): State<Actors>,
    Path(id): Path<String>,
    Json(req): Json<SessionPromptRequest>,
) -> impl IntoResponse {
    if req.prompt.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Prompt is required" })),
        );
    }

    let result = actors
        .sessions
        .ask(
            |reply| SessionsMsg::SessionPrompt(id, req.prompt, reply),
            ASK_TIMEOUT,
        )
        .await;

    match result {
        Ok(Ok(())) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true })),
        ),
        Ok(Err(e)) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": false, "error": e })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

async fn session_cancel(
    State(actors): State<Actors>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = actors
        .sessions
        .ask(|reply| SessionsMsg::SessionCancel(id, reply), ASK_TIMEOUT)
        .await;

    match result {
        Ok(Ok(())) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true })),
        ),
        Ok(Err(e)) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": false, "error": e })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct PermissionBody {
    response: String,
    #[serde(rename = "permissionId")]
    permission_id: String,
}

async fn session_permission(
    State(actors): State<Actors>,
    Path(id): Path<String>,
    Json(body): Json<PermissionBody>,
) -> impl IntoResponse {
    if body.response.is_empty() || !body.response.chars().all(|c| c.is_ascii_digit()) {
        return (
            StatusCode::BAD_REQUEST,
            JsonResponse(
                serde_json::json!({ "ok": false, "error": "Invalid response (expected number)" }),
            ),
        );
    }

    let result = actors
        .sessions
        .ask(
            |reply| SessionsMsg::SessionPermission(id, body.permission_id, body.response, reply),
            ASK_TIMEOUT,
        )
        .await;

    match result {
        Ok(Ok(())) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true })),
        ),
        Ok(Err(e)) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": false, "error": e })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

async fn session_restart(
    State(actors): State<Actors>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = actors
        .sessions
        .ask(|reply| SessionsMsg::SessionRestart(id, reply), ASK_TIMEOUT)
        .await;

    match result {
        Ok(Ok(session)) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "session": session })),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

async fn session_link(
    State(actors): State<Actors>,
    Path(id): Path<String>,
    Json(body): Json<LinkSessionRequest>,
) -> impl IntoResponse {
    let session = actors
        .sessions
        .ask(|reply| SessionsMsg::Link(id, body.claude_session_id, reply), ASK_TIMEOUT)
        .await
        .ok()
        .flatten();

    match session {
        Some(session) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "session": session })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Session not found" })),
        ),
    }
}

// ── Projects ────────────────────────────────────────────────────────────────

async fn list_projects(State(actors): State<Actors>) -> impl IntoResponse {
    let projects = actors
        .sessions
        .ask(SessionsMsg::ListProjects, ASK_TIMEOUT)
        .await
        .unwrap_or_default();
    JsonResponse(serde_json::json!({ "ok": true, "projects": projects }))
}

#[derive(Debug, Deserialize)]
struct AutocompleteQuery {
    q: Option<String>,
}

async fn autocomplete_projects(
    State(actors): State<Actors>,
    Query(query): Query<AutocompleteQuery>,
) -> impl IntoResponse {
    let partial = query.q.unwrap_or_default();
    let results = actors
        .sessions
        .ask(
            |reply| SessionsMsg::AutocompleteProjects(partial, reply),
            ASK_TIMEOUT,
        )
        .await
        .unwrap_or_default();
    JsonResponse(serde_json::json!({ "ok": true, "results": results }))
}

async fn delete_project(
    State(actors): State<Actors>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    info!("Removing project: {path}");
    let _ = actors
        .sessions
        .cast(SessionsMsg::RemoveProject(path));
    JsonResponse(serde_json::json!({ "ok": true }))
}

// ── Tiles ───────────────────────────────────────────────────────────────────

async fn list_tiles(State(actors): State<Actors>) -> impl IntoResponse {
    let tiles = actors
        .sessions
        .ask(SessionsMsg::ListTiles, ASK_TIMEOUT)
        .await
        .unwrap_or_default();
    JsonResponse(serde_json::json!({ "ok": true, "tiles": tiles }))
}

async fn create_tile(
    State(actors): State<Actors>,
    Json(req): Json<CreateTextTileRequest>,
) -> impl IntoResponse {
    if req.text.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Missing text or position" })),
        );
    }

    let tile = actors
        .sessions
        .ask(|reply| SessionsMsg::CreateTile(req, reply), ASK_TIMEOUT)
        .await;

    match tile {
        Ok(tile) => (
            StatusCode::CREATED,
            JsonResponse(serde_json::json!({ "ok": true, "tile": tile })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

async fn update_tile(
    State(actors): State<Actors>,
    Path(id): Path<String>,
    Json(req): Json<UpdateTextTileRequest>,
) -> impl IntoResponse {
    let tile = actors
        .sessions
        .ask(|reply| SessionsMsg::UpdateTile(id, req, reply), ASK_TIMEOUT)
        .await
        .ok()
        .flatten();

    match tile {
        Some(tile) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "tile": tile })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Tile not found" })),
        ),
    }
}

async fn delete_tile(
    State(actors): State<Actors>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let found = actors
        .sessions
        .ask(|reply| SessionsMsg::DeleteTile(id, reply), ASK_TIMEOUT)
        .await
        .unwrap_or(false);

    if found {
        (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            JsonResponse(serde_json::json!({ "ok": false, "error": "Tile not found" })),
        )
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn gethostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

// ── Tentacles target management (direct registry access) ────────────────────

async fn get_registry(actors: &Actors, session_id: &str) -> Result<crate::tentacles::targets::TargetRegistry, (StatusCode, JsonResponse<serde_json::Value>)> {
    actors
        .sessions
        .ask(
            |reply| SessionsMsg::GetTentaclesRegistry(session_id.to_string(), reply),
            ASK_TIMEOUT,
        )
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, JsonResponse(serde_json::json!({"ok": false, "error": "Timeout"}))))?
        .ok_or_else(|| (StatusCode::BAD_REQUEST, JsonResponse(serde_json::json!({"ok": false, "error": "Tentacles not enabled for this session"}))))
}

async fn list_targets(
    State(actors): State<Actors>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match get_registry(&actors, &id).await {
        Ok(registry) => JsonResponse(serde_json::json!({ "ok": true, "targets": registry.list().await })),
        Err((_, json)) => json,
    }
}

async fn add_target(
    State(actors): State<Actors>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let registry = match get_registry(&actors, &id).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };

    match crate::tentacles::parse_target_json(&body).await {
        Ok((info, target)) => {
            registry.add(info, target).await;
            JsonResponse(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, JsonResponse(serde_json::json!({ "ok": false, "error": e.to_string() }))).into_response(),
    }
}

async fn remove_target(
    State(actors): State<Actors>,
    Path((id, name)): Path<(String, String)>,
) -> impl IntoResponse {
    match get_registry(&actors, &id).await {
        Ok(registry) => {
            if registry.remove(&name).await {
                JsonResponse(serde_json::json!({ "ok": true }))
            } else {
                JsonResponse(serde_json::json!({ "ok": false, "error": "Target not found" }))
            }
        }
        Err((_, json)) => json,
    }
}

// ── Docker helpers ──────────────────────────────────────────────────────────

async fn list_docker_containers() -> impl IntoResponse {
    match tokio::process::Command::new("docker")
        .args(["ps", "--format", "{{.Names}}\t{{.Image}}\t{{.Status}}"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let containers: Vec<serde_json::Value> = stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|line| {
                    let parts: Vec<&str> = line.splitn(3, '\t').collect();
                    serde_json::json!({
                        "name": parts.first().unwrap_or(&""),
                        "image": parts.get(1).unwrap_or(&""),
                        "status": parts.get(2).unwrap_or(&""),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                JsonResponse(serde_json::json!({ "ok": true, "containers": containers })),
            )
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr);
            (
                StatusCode::OK,
                JsonResponse(serde_json::json!({ "ok": true, "containers": [], "warning": err.to_string() })),
            )
        }
        Err(_) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "containers": [], "warning": "docker not found" })),
        ),
    }
}

async fn list_docker_images() -> impl IntoResponse {
    match tokio::process::Command::new("docker")
        .args(["images", "--format", "{{.Repository}}:{{.Tag}}\t{{.Size}}"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let images: Vec<serde_json::Value> = stdout
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with("<none>"))
                .map(|line| {
                    let parts: Vec<&str> = line.splitn(2, '\t').collect();
                    serde_json::json!({
                        "name": parts.first().unwrap_or(&""),
                        "size": parts.get(1).unwrap_or(&""),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                JsonResponse(serde_json::json!({ "ok": true, "images": images })),
            )
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr);
            (
                StatusCode::OK,
                JsonResponse(serde_json::json!({ "ok": true, "images": [], "warning": err.to_string() })),
            )
        }
        Err(_) => (
            StatusCode::OK,
            JsonResponse(serde_json::json!({ "ok": true, "images": [], "warning": "docker not found" })),
        ),
    }
}

pub fn is_valid_tmux_session(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
