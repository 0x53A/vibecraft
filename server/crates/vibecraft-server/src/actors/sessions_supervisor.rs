//! SessionsSupervisor actor: manages the collection of session actors,
//! the claude-to-managed mapping, tiles, projects, and persistence.

use std::collections::HashMap;
use std::time::Duration;

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort, SupervisionEvent};
use serde::Deserialize;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Wrap a string in single quotes for shell, escaping any internal single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

use crate::config::Config;
use crate::projects::ProjectsManager;
use crate::tmux;
use crate::types::{
    ClaudeEvent, CreateSessionRequest, CreateTextTileRequest, KnownProject,
    ManagedSession, ServerMessage, SessionStatus, TextTile, UpdateSessionRequest,
    UpdateTextTileRequest,
};

use super::hub::HubMsg;
use super::session::{SessionActor, SessionArgs, SessionMsg};

// ── Messages ────────────────────────────────────────────────────────────────

pub enum SessionsMsg {
    // ── Session CRUD ────────────────────────────────────────────────────
    List(RpcReplyPort<Vec<ManagedSession>>),
    Get(String, RpcReplyPort<Option<ManagedSession>>),
    Create(CreateSessionRequest, RpcReplyPort<Result<ManagedSession, String>>),
    Delete(String, RpcReplyPort<bool>),
    Update(String, UpdateSessionRequest, RpcReplyPort<Option<ManagedSession>>),
    Link(String, String, RpcReplyPort<Option<ManagedSession>>), // managed_id, claude_session_id
    LinkByTmux(String, String), // tmux_session_name, claude_session_id

    // ── Event routing ───────────────────────────────────────────────────
    RouteEvent(ClaudeEvent),

    // ── From SessionActors (cache update) ───────────────────────────────
    SessionUpdated(String, ManagedSession),

    // ── Session actions ─────────────────────────────────────────────────
    SessionPrompt(String, String, RpcReplyPort<Result<(), String>>),
    SessionCancel(String, RpcReplyPort<Result<(), String>>),
    SessionPermission(String, String, String, RpcReplyPort<Result<(), String>>),
    /// Fire-and-forget permission response (from WS client message).
    SessionPermissionCast(String, String, String),
    SessionRestart(String, RpcReplyPort<Result<ManagedSession, String>>),
    // ── Health ──────────────────────────────────────────────────────────
    HealthCheck,
    Refresh(RpcReplyPort<Vec<ManagedSession>>),

    // ── Tiles ───────────────────────────────────────────────────────────
    ListTiles(RpcReplyPort<Vec<TextTile>>),
    CreateTile(CreateTextTileRequest, RpcReplyPort<TextTile>),
    UpdateTile(String, UpdateTextTileRequest, RpcReplyPort<Option<TextTile>>),
    DeleteTile(String, RpcReplyPort<bool>),

    // ── Projects ────────────────────────────────────────────────────────
    ListProjects(RpcReplyPort<Vec<KnownProject>>),
    AutocompleteProjects(String, RpcReplyPort<Vec<String>>),
    RemoveProject(String),

    // ── Active session IDs (for WS history filtering) ───────────────────
    GetActiveClaudeIds(RpcReplyPort<std::collections::HashSet<String>>),

    // ── Adopt an existing tmux session as a managed session ──────────────
    AdoptTmuxSession {
        tmux_session: String,
        cwd: String,
        reply: RpcReplyPort<Result<ManagedSession, String>>,
    },

    // ── Tentacles ─────────────────────────────────────────────────────────
    GetTentaclesRegistry(String, RpcReplyPort<Option<crate::tentacles::targets::TargetRegistry>>),
}

// ── Actor ───────────────────────────────────────────────────────────────────

pub struct SessionsSupervisorActor;

pub struct SupervisorState {
    /// SessionActor refs, keyed by managed session ID.
    actors: HashMap<String, ActorRef<SessionMsg>>,
    /// Cached snapshots from session actors, keyed by managed session ID.
    cache: HashMap<String, ManagedSession>,
    /// Maps Claude session ID → managed session ID.
    claude_to_managed: HashMap<String, String>,
    session_counter: u32,
    hub: ActorRef<HubMsg>,
    config: Config,
    // Tiles
    tiles: HashMap<String, TextTile>,
    // Projects
    projects_manager: ProjectsManager,
    // Tentacles registries, keyed by managed session ID
    tentacles_registries: HashMap<String, crate::tentacles::targets::TargetRegistry>,
}

pub struct SupervisorArgs {
    pub hub: ActorRef<HubMsg>,
    pub config: Config,
}


impl Actor for SessionsSupervisorActor {
    type Msg = SessionsMsg;
    type State = SupervisorState;
    type Arguments = SupervisorArgs;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let mut state = SupervisorState {
            actors: HashMap::new(),
            cache: HashMap::new(),
            claude_to_managed: HashMap::new(),
            session_counter: 0,
            hub: args.hub.clone(),
            config: args.config.clone(),
            tiles: HashMap::new(),
            projects_manager: ProjectsManager::new(),
            tentacles_registries: HashMap::new(),
        };

        // Load persisted sessions
        load_sessions(&args.config, &mut state).await;

        // Spawn a SessionActor for each loaded session, restarting tentacles if needed
        for session in state.cache.values().cloned().collect::<Vec<_>>() {
            let id = session.id.clone();

            // Restart tentacles MCP server on the same port if session had it
            let mut tentacles_handle = None;
            if let Some(ref ti) = session.tentacles {
                if ti.enabled {
                    match crate::tentacles::start(&ti.targets, Some(ti.port)).await {
                        Ok(handle) => {
                            info!(
                                "Tentacles restarted for session {} on port {}",
                                &id[..8.min(id.len())],
                                handle.port
                            );
                            // Write MCP config so the existing Claude process can reconnect
                            let _ = write_mcp_config(
                                &id,
                                session.spawn_flags.as_ref(),
                                Some(handle.port),
                            );
                            state.tentacles_registries.insert(id.clone(), handle.registry.clone());
                            tentacles_handle = Some(handle);
                        }
                        Err(e) => {
                            warn!("Failed to restart tentacles for session {}: {e}", &id[..8.min(id.len())]);
                        }
                    }
                }
            }

            let skip_permissions = session
                .spawn_flags
                .as_ref()
                .and_then(|f| f.skip_permissions)
                .unwrap_or(false);

            match Actor::spawn_linked(
                Some(format!("session-{}", &id[..8.min(id.len())])),
                SessionActor,
                SessionArgs {
                    session,
                    hub: args.hub.clone(),
                    supervisor: myself.clone(),
                    exec_path: args.config.exec_path.clone(),
                    working_timeout_ms: args.config.working_timeout_ms,
                    working_timeout_check_interval_ms: args.config.working_timeout_check_interval_ms,
                    skip_permissions,
                    tentacles_handle,
                },
                myself.get_cell(),
            )
            .await
            {
                Ok((actor_ref, _)) => {
                    state.actors.insert(id, actor_ref);
                }
                Err(e) => {
                    error!("Failed to spawn session actor: {e}");
                }
            }
        }

        // Load tiles
        load_tiles(&args.config, &mut state).await;

        // Schedule health check
        myself.send_after(Duration::from_secs(2), || SessionsMsg::HealthCheck);

        Ok(state)
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            // ── Session CRUD ────────────────────────────────────────────
            SessionsMsg::List(reply) => {
                let sessions: Vec<ManagedSession> = state.cache.values().cloned().collect();
                let _ = reply.send(sessions);
            }

            SessionsMsg::Get(id, reply) => {
                let _ = reply.send(state.cache.get(&id).cloned());
            }

            SessionsMsg::Create(req, reply) => {
                let result = self.create_session(myself.clone(), req, state).await;
                let _ = reply.send(result);
            }

            SessionsMsg::Delete(id, reply) => {
                let found = self.delete_session(&id, state).await;
                let _ = reply.send(found);
                if found {
                    self.broadcast_sessions(state);
                    self.save_sessions(state).await;
                }
            }

            SessionsMsg::Update(id, updates, reply) => {
                if let Some(actor_ref) = state.actors.get(&id) {
                    let _ = actor_ref.cast(SessionMsg::Update {
                        name: updates.name,
                        zone_position: updates.zone_position,
                    });
                    // Reply with current cached (will be updated on SessionUpdated)
                    let _ = reply.send(state.cache.get(&id).cloned());
                } else {
                    let _ = reply.send(None);
                }
            }

            SessionsMsg::Link(managed_id, claude_session_id, reply) => {
                state
                    .claude_to_managed
                    .insert(claude_session_id.clone(), managed_id.clone());
                if let Some(actor_ref) = state.actors.get(&managed_id) {
                    let _ = actor_ref.cast(SessionMsg::Link(claude_session_id));
                }
                self.save_sessions(state).await;
                self.broadcast_sessions(state);
                let _ = reply.send(state.cache.get(&managed_id).cloned());
            }

            SessionsMsg::LinkByTmux(tmux_name, claude_session_id) => {
                // Already linked? Skip.
                if !state.claude_to_managed.contains_key(&claude_session_id) {
                    // Find managed session by tmux session name
                    if let Some(managed) = state.cache.values().find(|s| s.tmux_session == tmux_name) {
                        let managed_id = managed.id.clone();
                        info!(
                            "Auto-linking claude {} to managed {} via tmux {}",
                            &claude_session_id[..8.min(claude_session_id.len())],
                            &managed_id[..8.min(managed_id.len())],
                            tmux_name,
                        );
                        state
                            .claude_to_managed
                            .insert(claude_session_id.clone(), managed_id.clone());
                        if let Some(actor_ref) = state.actors.get(&managed_id) {
                            let _ = actor_ref.cast(SessionMsg::Link(claude_session_id));
                        }
                        self.save_sessions(state).await;
                        self.broadcast_sessions(state);
                    }
                }
            }

            // ── Event routing ───────────────────────────────────────────
            SessionsMsg::RouteEvent(event) => {
                let claude_sid = event.session_id().to_string();
                if let Some(managed_id) = state.claude_to_managed.get(&claude_sid) {
                    if let Some(actor_ref) = state.actors.get(managed_id) {
                        let _ = actor_ref.cast(SessionMsg::HandleEvent(event));
                    }
                }
            }

            // ── Cache updates from children ─────────────────────────────
            SessionsMsg::SessionUpdated(id, session) => {
                state.cache.insert(id, session);
                self.broadcast_sessions(state);
                self.save_sessions(state).await;
            }

            // ── Session actions ─────────────────────────────────────────
            SessionsMsg::SessionPrompt(id, text, reply) => {
                if let Some(actor_ref) = state.actors.get(&id) {
                    let _ = actor_ref.cast(SessionMsg::SendPrompt(text, reply));
                } else {
                    let _ = reply.send(Err("Session not found".into()));
                }
            }

            SessionsMsg::SessionCancel(id, reply) => {
                if let Some(actor_ref) = state.actors.get(&id) {
                    let _ = actor_ref.cast(SessionMsg::Cancel(reply));
                } else {
                    let _ = reply.send(Err("Session not found".into()));
                }
            }

            SessionsMsg::SessionPermission(id, perm_id, response, reply) => {
                if let Some(actor_ref) = state.actors.get(&id) {
                    let _ = actor_ref.cast(SessionMsg::PermissionResponse(perm_id, response));
                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err("Session not found".into()));
                }
            }

            SessionsMsg::SessionPermissionCast(id, perm_id, response) => {
                if let Some(actor_ref) = state.actors.get(&id) {
                    let _ = actor_ref.cast(SessionMsg::PermissionResponse(perm_id, response));
                }
            }

            SessionsMsg::SessionRestart(id, reply) => {
                if let Some(actor_ref) = state.actors.get(&id) {
                    let _ = actor_ref.cast(SessionMsg::Restart(reply));
                    // Also clear old linkings
                    state
                        .claude_to_managed
                        .retain(|_, mid| mid != &id);
                } else {
                    let _ = reply.send(Err("Session not found".into()));
                }
            }

            // ── Health ──────────────────────────────────────────────────
            SessionsMsg::HealthCheck => {
                self.do_health_check(state).await;
                // Reschedule
                myself.send_after(Duration::from_secs(5), || SessionsMsg::HealthCheck);
            }

            SessionsMsg::Refresh(reply) => {
                self.do_health_check(state).await;
                let sessions: Vec<ManagedSession> = state.cache.values().cloned().collect();
                let _ = reply.send(sessions);
            }

            // ── Tiles ───────────────────────────────────────────────────
            SessionsMsg::ListTiles(reply) => {
                let tiles: Vec<TextTile> = state.tiles.values().cloned().collect();
                let _ = reply.send(tiles);
            }

            SessionsMsg::CreateTile(req, reply) => {
                let tile = TextTile {
                    id: Uuid::new_v4().to_string(),
                    text: req.text,
                    position: req.position,
                    color: req.color,
                    created_at: now_ms(),
                };
                state.tiles.insert(tile.id.clone(), tile.clone());
                info!(
                    "Created text tile: \"{}\" at ({}, {})",
                    tile.text, tile.position.q, tile.position.r
                );
                save_tiles(&state.config, &state.tiles).await;
                self.broadcast_tiles(state);
                let _ = reply.send(tile);
            }

            SessionsMsg::UpdateTile(id, req, reply) => {
                if let Some(tile) = state.tiles.get_mut(&id) {
                    if let Some(text) = req.text {
                        tile.text = text;
                    }
                    if let Some(position) = req.position {
                        tile.position = position;
                    }
                    if let Some(color) = req.color {
                        tile.color = Some(color);
                    }
                    let tile = tile.clone();
                    info!("Updated text tile: \"{}\"", tile.text);
                    save_tiles(&state.config, &state.tiles).await;
                    self.broadcast_tiles(state);
                    let _ = reply.send(Some(tile));
                } else {
                    let _ = reply.send(None);
                }
            }

            SessionsMsg::DeleteTile(id, reply) => {
                if let Some(tile) = state.tiles.remove(&id) {
                    info!("Deleted text tile: \"{}\"", tile.text);
                    save_tiles(&state.config, &state.tiles).await;
                    self.broadcast_tiles(state);
                    let _ = reply.send(true);
                } else {
                    let _ = reply.send(false);
                }
            }

            // ── Projects ────────────────────────────────────────────────
            SessionsMsg::ListProjects(reply) => {
                let _ = reply.send(state.projects_manager.get_projects());
            }

            SessionsMsg::AutocompleteProjects(query, reply) => {
                let results = state.projects_manager.autocomplete(&query, 15);
                let _ = reply.send(results);
            }

            SessionsMsg::RemoveProject(path) => {
                state.projects_manager.remove_project(&path);
            }

            // ── Active Claude IDs ───────────────────────────────────────
            SessionsMsg::GetActiveClaudeIds(reply) => {
                // Only include non-offline sessions to avoid leaking
                // old history from dead Claude Code processes.
                let ids: std::collections::HashSet<String> = state
                    .cache
                    .values()
                    .filter(|s| s.status != SessionStatus::Offline)
                    .filter_map(|s| s.claude_session_id.clone())
                    .collect();
                let _ = reply.send(ids);
            }

            // ── Adopt existing tmux session ────────────────────────────
            SessionsMsg::AdoptTmuxSession { tmux_session, cwd, reply } => {
                let result = self.adopt_tmux_session(
                    &myself, state, tmux_session, cwd,
                ).await;
                let _ = reply.send(result);
            }

            SessionsMsg::GetTentaclesRegistry(id, reply) => {
                let _ = reply.send(state.tentacles_registries.get(&id).cloned());
            }
        }
        Ok(())
    }

    fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: SupervisionEvent,
        _state: &mut Self::State,
    ) -> impl std::future::Future<Output = Result<(), ActorProcessingErr>> + Send {
        match &msg {
            SupervisionEvent::ActorTerminated(cell, _, reason) => {
                warn!(
                    "Child actor terminated: {} (id={}), reason: {:?}",
                    cell.get_name().unwrap_or_default(),
                    cell.get_id(),
                    reason
                );
            }
            SupervisionEvent::ActorFailed(cell, err) => {
                error!(
                    "Child actor failed: {} (id={}), error: {}",
                    cell.get_name().unwrap_or_default(),
                    cell.get_id(),
                    err
                );
            }
            _ => {}
        }
        async { Ok(()) }
    }

    fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        _state: &mut Self::State,
    ) -> impl std::future::Future<Output = Result<(), ActorProcessingErr>> + Send {
        error!("SessionsSupervisor actor stopped!");
        async { Ok(()) }
    }
}

impl SessionsSupervisorActor {
    fn broadcast_sessions(&self, state: &SupervisorState) {
        let sessions: Vec<ManagedSession> = state.cache.values().cloned().collect();
        let _ = state
            .hub
            .cast(HubMsg::Broadcast(ServerMessage::Sessions(sessions)));
    }

    fn broadcast_tiles(&self, state: &SupervisorState) {
        let tiles: Vec<TextTile> = state.tiles.values().cloned().collect();
        let _ = state
            .hub
            .cast(HubMsg::Broadcast(ServerMessage::TextTiles(tiles)));
    }

    async fn save_sessions(&self, state: &SupervisorState) {
        let sessions: Vec<&ManagedSession> = state.cache.values().collect();
        let map: Vec<(&String, &String)> = state.claude_to_managed.iter().collect();
        let data = serde_json::json!({
            "sessions": sessions,
            "claudeToManagedMap": map,
            "sessionCounter": state.session_counter,
        });

        match serde_json::to_string_pretty(&data) {
            Ok(json) => {
                if let Some(parent) = state.config.sessions_file.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = tokio::fs::write(&state.config.sessions_file, json).await {
                    error!("Failed to save sessions: {e}");
                }
            }
            Err(e) => {
                error!("Failed to serialize sessions: {e}");
            }
        }
    }

    async fn create_session(
        &self,
        myself: ActorRef<SessionsMsg>,
        req: CreateSessionRequest,
        state: &mut SupervisorState,
    ) -> Result<ManagedSession, String>
    where
        Self: Sized,
    {
        let id = Uuid::new_v4().to_string();
        state.session_counter += 1;
        let name = req
            .name
            .unwrap_or_else(|| format!("Claude {}", state.session_counter));
        let tmux_session = format!("vibecraft-{}", &Uuid::new_v4().to_string()[..8]);

        let cwd = req.cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "/tmp".into())
        });

        // Expand ~ and create directory if it doesn't exist
        let cwd = {
            let expanded = if let Some(rest) = cwd.strip_prefix("~/") {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                format!("{home}/{rest}")
            } else if cwd == "~" {
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
            } else {
                cwd
            };
            let path = std::path::Path::new(&expanded);
            if !path.exists() {
                std::fs::create_dir_all(path)
                    .map_err(|e| format!("Failed to create directory {expanded}: {e}"))?;
                info!("Created directory: {expanded}");
            }
            expanded
        };

        // Build claude command
        let flags = req.flags.as_ref();
        let skip_permissions = flags.and_then(|f| f.skip_permissions).unwrap_or(true);
        let mut claude_args: Vec<String> = Vec::new();

        // Resume takes precedence over continue
        if let Some(ref resume_id) = req.resume {
            claude_args.push("--resume".into());
            claude_args.push(resume_id.clone());
        } else if flags.and_then(|f| f.continue_session).unwrap_or(true) {
            claude_args.push("-c".into());
        }
        if skip_permissions {
            claude_args.push("--permission-mode=bypassPermissions".into());
            claude_args.push("--dangerously-skip-permissions".into());
        }
        if flags.and_then(|f| f.chrome).unwrap_or(false) {
            claude_args.push("--chrome".into());
        }

        // System prompt mode
        match flags.and_then(|f| f.system_prompt_mode.as_deref()) {
            Some("append") => {
                if let Some(text) = flags.and_then(|f| f.system_prompt_text.as_ref()) {
                    if !text.is_empty() {
                        claude_args.push("--append-system-prompt".into());
                        claude_args.push(shell_quote(text));
                    }
                }
            }
            Some("replace") => {
                let text = flags.and_then(|f| f.system_prompt_text.as_ref()).cloned().unwrap_or_default();
                let text = if text.trim().is_empty() { ".".into() } else { text };
                claude_args.push("--system-prompt".into());
                claude_args.push(shell_quote(&text));
            }
            _ => {} // "default" or None — no extra args
        }

        // ── Tentacles: start in-process MCP proxy if enabled ────────────────
        // (must happen before --tools, since tentacles overrides tool selection)
        let tentacles_config = flags.and_then(|f| f.tentacles.as_ref()).filter(|t| t.enabled);
        let mut tentacles_info: Option<crate::types::TentaclesSessionInfo> = None;
        let mut tentacles_handle: Option<crate::tentacles::TentaclesHandle> = None;

        if let Some(tc) = tentacles_config {
            match crate::tentacles::start(&tc.targets, None).await {
                Ok(handle) => {
                    info!("Tentacles started in-process on port {}", handle.port);
                    tentacles_info = Some(crate::types::TentaclesSessionInfo {
                        enabled: true,
                        port: handle.port,
                        targets: tc.targets.clone(),
                    });
                    // Store registry in supervisor for direct access from routes
                    state.tentacles_registries.insert(id.clone(), handle.registry.clone());
                    tentacles_handle = Some(handle);
                }
                Err(e) => {
                    tracing::warn!("Failed to start tentacles: {e}");
                }
            }
        }

        // ── MCP config: combine user servers + tentacles ────────────────────
        let tentacles_port = tentacles_info.as_ref().map(|ti| ti.port);
        match write_mcp_config(&id, flags, tentacles_port) {
            Ok(mcp_path) => {
                claude_args.push("--mcp-config".into());
                claude_args.push(mcp_path);
            }
            Err(_) => {} // No MCP servers to configure
        }

        // ── Tool restrictions ────────────────────────────────────────────────
        // MCP tools (mcp__tentacles__*, user MCP servers) are always available
        // regardless of --tools. User's tool selections apply to built-in tools only.
        if let Some(tools) = flags.and_then(|f| f.tools.as_ref()) {
            let has_lsp = tools.iter().any(|t| t == "LSP");
            let non_lsp: Vec<&str> = tools.iter().filter(|t| *t != "LSP").map(|s| s.as_str()).collect();
            if tools.is_empty() {
                claude_args.push("--tools".into());
                claude_args.push("\"\"".into());
                claude_args.push("--disallowed-tools".into());
                claude_args.push("LSP".into());
            } else {
                claude_args.push("--tools".into());
                claude_args.push(non_lsp.join(","));
                if !has_lsp {
                    claude_args.push("--disallowed-tools".into());
                    claude_args.push("LSP".into());
                }
            }
        }

        let exec_path = state.config.exec_path.clone();
        // Pass managed session ID as env var so the hook can auto-link
        let memory_enabled = flags.and_then(|f| f.memory).unwrap_or(true);

        // Build shell command with proper quoting to prevent injection
        let mut env_parts = Vec::new();
        if !memory_enabled {
            env_parts.push("CLAUDE_CODE_DISABLE_AUTO_MEMORY=1".to_string());
        }
        env_parts.push(format!("VIBECRAFT_MANAGED_SESSION_ID={}", shell_quote(&id)));
        env_parts.push(format!("PATH={}", shell_quote(&exec_path)));

        let mut spawn_parts = env_parts;
        spawn_parts.push("claude".to_string());
        for arg in &claude_args {
            spawn_parts.push(shell_quote(arg));
        }
        let spawn_cmd = spawn_parts.join(" ");
        info!("Spawning: {spawn_cmd}");

        let result = tokio::process::Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &tmux_session,
                "-c",
                &cwd,
                &spawn_cmd,
            ])
            .env("PATH", &exec_path)
            .output()
            .await
            .map_err(|e| format!("Failed to spawn session: {e}"))?;

        if !result.status.success() {
            let err = String::from_utf8_lossy(&result.stderr);
            return Err(format!("Failed to spawn session: {err}"));
        }

        let session = ManagedSession {
            id: id.clone(),
            name: name.clone(),
            tmux_session,
            status: SessionStatus::Idle,
            claude_session_id: None,
            created_at: now_ms(),
            last_activity: now_ms(),
            cwd: Some(cwd),
            current_tool: None,
            tokens: None,
            git_status: None,
            zone_position: None,
            pending_permission: None,
            tentacles: tentacles_info,
            spawn_flags: req.flags.clone(),
        };

        // Spawn SessionActor
        match Actor::spawn_linked(
            Some(format!("session-{}", &id[..8.min(id.len())])),
            SessionActor,
            SessionArgs {
                session: session.clone(),
                hub: state.hub.clone(),
                supervisor: myself.clone(),
                exec_path: state.config.exec_path.clone(),
                working_timeout_ms: state.config.working_timeout_ms,
                working_timeout_check_interval_ms: state.config.working_timeout_check_interval_ms,
                skip_permissions,
                tentacles_handle,
            },
            myself.get_cell(),
        )
        .await
        {
            Ok((actor_ref, _)) => {
                state.actors.insert(id.clone(), actor_ref);
                state.cache.insert(id, session.clone());
                info!("Created session: {name} ({})", &session.id[..8.min(session.id.len())]);
                self.broadcast_sessions(state);
                self.save_sessions(state).await;
                Ok(session)
            }
            Err(e) => Err(format!("Failed to spawn session actor: {e}")),
        }
    }

    async fn delete_session(&self, id: &str, state: &mut SupervisorState) -> bool {
        if let Some(actor_ref) = state.actors.remove(id) {
            actor_ref.stop(Some("deleted".to_string()));
        }

        if let Some(session) = state.cache.remove(id) {
            // Kill tmux session
            if tmux::validate_tmux_session(&session.tmux_session).is_ok() {
                let _ = tokio::process::Command::new("tmux")
                    .args(["kill-session", "-t", &session.tmux_session])
                    .env("PATH", &state.config.exec_path)
                    .output()
                    .await;
            }

            // Clean up mappings
            state.claude_to_managed.retain(|_, mid| mid != id);
            state.tentacles_registries.remove(id);

            info!(
                "Deleted session: {} ({})",
                session.name,
                &id[..8.min(id.len())]
            );
            true
        } else {
            false
        }
    }

    async fn do_health_check(&self, state: &mut SupervisorState) {
        let exec_path = &state.config.exec_path;
        let result = tokio::process::Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .env("PATH", exec_path)
            .output()
            .await;

        let active_sessions: std::collections::HashSet<String> = match result {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .trim()
                .lines()
                .map(|s| s.to_string())
                .collect(),
            _ => std::collections::HashSet::new(),
        };

        // Send health update to each session actor
        for (id, actor_ref) in &state.actors {
            if let Some(session) = state.cache.get(id) {
                let alive = active_sessions.contains(&session.tmux_session);
                let _ = actor_ref.cast(SessionMsg::HealthUpdate { alive });
            }
        }
    }

    /// Adopt an existing tmux session as a managed session without spawning
    /// a new tmux process. Used on startup to reclaim orphaned sessions.
    async fn adopt_tmux_session(
        &self,
        myself: &ActorRef<SessionsMsg>,
        state: &mut SupervisorState,
        tmux_session: String,
        cwd: String,
    ) -> Result<ManagedSession, String> {
        // Check if already tracked
        if state.cache.values().any(|s| s.tmux_session == tmux_session) {
            return Err(format!("Session {tmux_session} is already tracked"));
        }

        let id = Uuid::new_v4().to_string();
        state.session_counter += 1;

        // Derive a friendly name from the tmux session name
        let name = format!("Claude {}", state.session_counter);

        let session = ManagedSession {
            id: id.clone(),
            name: name.clone(),
            tmux_session: tmux_session.clone(),
            status: SessionStatus::Idle,
            claude_session_id: None,
            created_at: now_ms(),
            last_activity: now_ms(),
            cwd: Some(cwd),
            current_tool: None,
            tokens: None,
            git_status: None,
            zone_position: None,
            pending_permission: None,
            tentacles: None,
            spawn_flags: None,
        };

        // Spawn SessionActor
        match Actor::spawn_linked(
            Some(format!("session-{}", &id[..8.min(id.len())])),
            SessionActor,
            SessionArgs {
                session: session.clone(),
                hub: state.hub.clone(),
                supervisor: myself.clone(),
                exec_path: state.config.exec_path.clone(),
                working_timeout_ms: state.config.working_timeout_ms,
                working_timeout_check_interval_ms: state.config.working_timeout_check_interval_ms,
                skip_permissions: false,
                tentacles_handle: None,
            },
            myself.get_cell(),
        )
        .await
        {
            Ok((actor_ref, _)) => {
                state.actors.insert(id.clone(), actor_ref);
                state.cache.insert(id.clone(), session.clone());
            }
            Err(e) => {
                return Err(format!("Failed to spawn session actor: {e}"));
            }
        }

        // Set VIBECRAFT_MANAGED_SESSION_ID in the tmux session so future
        // hooks can auto-link events to this managed session.
        let _ = tokio::process::Command::new("tmux")
            .args([
                "set-environment",
                "-t",
                &tmux_session,
                "VIBECRAFT_MANAGED_SESSION_ID",
                &id,
            ])
            .env("PATH", &state.config.exec_path)
            .output()
            .await;

        self.save_sessions(state).await;
        self.broadcast_sessions(state);

        info!("Adopted tmux session: {tmux_session} as \"{name}\" ({})", &id[..8]);
        Ok(session)
    }
}

// ── Persistence helpers ─────────────────────────────────────────────────────

async fn load_sessions(config: &Config, state: &mut SupervisorState) {
    let path = &config.sessions_file;
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!("Failed to read sessions file: {e}");
            return;
        }
    };

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TsSessionsFile {
        sessions: Vec<ManagedSession>,
        #[serde(default)]
        claude_to_managed_map: Vec<(String, String)>,
        #[serde(default)]
        session_counter: u32,
    }

    let (session_list, map_entries, counter) =
        if let Ok(ts_data) = serde_json::from_str::<TsSessionsFile>(&contents) {
            (
                ts_data.sessions,
                ts_data.claude_to_managed_map,
                ts_data.session_counter,
            )
        } else if let Ok(list) = serde_json::from_str::<Vec<ManagedSession>>(&contents) {
            (list, Vec::new(), 0)
        } else {
            warn!("Failed to parse sessions file");
            return;
        };

    for mut s in session_list {
        s.status = SessionStatus::Offline;
        s.current_tool = None;
        // Keep claude_session_id — if the tmux session is still alive,
        // the health check will mark it non-offline and events will route correctly.
        // Stale links are handled by filtering GetActiveClaudeIds to non-offline sessions.
        if let Some(ref cid) = s.claude_session_id {
            state
                .claude_to_managed
                .insert(cid.clone(), s.id.clone());
        }
        state.cache.insert(s.id.clone(), s);
    }

    // Restore explicit map entries from file
    for (claude_id, managed_id) in map_entries {
        state.claude_to_managed.insert(claude_id, managed_id);
    }

    state.session_counter = counter;
    info!(
        "Loaded {} sessions from {}",
        state.cache.len(),
        path.display()
    );
}

async fn load_tiles(config: &Config, state: &mut SupervisorState) {
    let path = &config.tiles_file;
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => match serde_json::from_str::<Vec<TextTile>>(&contents) {
            Ok(list) => {
                for t in list {
                    state.tiles.insert(t.id.clone(), t);
                }
            }
            Err(e) => {
                warn!("Failed to parse tiles file: {e}");
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            warn!("Failed to read tiles file: {e}");
        }
    }
}

async fn save_tiles(config: &Config, tiles: &HashMap<String, TextTile>) {
    let values: Vec<&TextTile> = tiles.values().collect();
    match serde_json::to_string_pretty(&values) {
        Ok(json) => {
            if let Some(parent) = config.tiles_file.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            if let Err(e) = tokio::fs::write(&config.tiles_file, json).await {
                error!("Failed to save tiles: {e}");
            }
        }
        Err(e) => {
            error!("Failed to serialize tiles: {e}");
        }
    }
}

/// Write MCP config JSON for a session, combining user MCP servers + optional tentacles.
/// Returns the path on success.
fn write_mcp_config(
    session_id: &str,
    flags: Option<&crate::types::SessionFlags>,
    tentacles_port: Option<u16>,
) -> Result<String, String> {
    let mut mcp_servers_map = serde_json::Map::new();

    // User-defined MCP servers (stdio)
    if let Some(mcp_servers) = flags.and_then(|f| f.mcp_servers.as_ref()) {
        for server in mcp_servers {
            let mut entry = serde_json::Map::new();
            entry.insert("command".into(), serde_json::Value::String(server.command.clone()));
            if let Some(ref args) = server.args {
                entry.insert(
                    "args".into(),
                    serde_json::Value::Array(
                        args.iter().map(|a| serde_json::Value::String(a.clone())).collect::<Vec<_>>(),
                    ),
                );
            }
            mcp_servers_map.insert(server.name.clone(), serde_json::Value::Object(entry));
        }
    }

    // Tentacles MCP server (HTTP)
    if let Some(port) = tentacles_port {
        let mut entry = serde_json::Map::new();
        entry.insert("type".into(), serde_json::Value::String("http".into()));
        entry.insert(
            "url".into(),
            serde_json::Value::String(format!("http://127.0.0.1:{port}/mcp")),
        );
        mcp_servers_map.insert("tentacles".into(), serde_json::Value::Object(entry));
    }

    if mcp_servers_map.is_empty() {
        return Err("No MCP servers to write".into());
    }

    let mcp_dir = format!(
        "{}/mcp",
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()) + "/.vibecraft/data"
    );
    let _ = std::fs::create_dir_all(&mcp_dir);
    let mcp_path = format!("{mcp_dir}/{session_id}.json");
    let config = serde_json::json!({ "mcpServers": mcp_servers_map });
    std::fs::write(&mcp_path, config.to_string())
        .map_err(|e| format!("Failed to write MCP config {mcp_path}: {e}"))?;
    Ok(mcp_path)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
