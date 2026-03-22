//! SessionActor: one per managed session.
//!
//! Owns its ManagedSession state, token tracking, and spawns a TmuxPoller child.
//! Reports state changes to the SessionsSupervisor which handles persistence
//! and broadcasting.

use std::time::Duration;

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort, SupervisionEvent};
use tracing::{info, warn};
use uuid::Uuid;

use crate::git::GitStatusManager;
use crate::types::{
    ClaudeEvent, HexPosition, ManagedSession, PermissionOption, SessionStatus, TokenInfo,
};

use super::hub::HubMsg;
use super::sessions_supervisor::SessionsMsg;
use super::tmux_poller::{TmuxPollerActor, TmuxPollerArgs, TmuxPollerMsg};

// ── Messages ────────────────────────────────────────────────────────────────

pub enum SessionMsg {
    /// Route an event to this session (status transitions).
    HandleEvent(ClaudeEvent),
    /// Update session name and/or zone position.
    Update {
        name: Option<String>,
        zone_position: Option<HexPosition>,
    },
    /// Link a Claude session ID to this managed session.
    Link(String),
    /// Send a prompt to this session's tmux.
    SendPrompt(String, RpcReplyPort<Result<(), String>>),
    /// Send Ctrl+C to this session.
    Cancel(RpcReplyPort<Result<(), String>>),
    /// Respond to a permission prompt. (permission_id, response_number)
    PermissionResponse(String, String),
    /// Restart this session's tmux.
    Restart(RpcReplyPort<Result<ManagedSession, String>>),
    /// Health update from supervisor (tmux alive check).
    HealthUpdate { alive: bool },

    // From TmuxPoller child:
    /// Token count update.
    TokenUpdate { current: u64 },
    /// Permission prompt detected.
    PermissionDetected {
        tool: String,
        context: String,
        options: Vec<PermissionOption>,
    },
    /// Permission prompt resolved.
    PermissionResolved,

    /// Self-scheduled: check for working timeout.
    CheckWorkingTimeout,
    /// Self-scheduled: poll git status.
    PollGitStatus,
}

// ── Actor ───────────────────────────────────────────────────────────────────

pub struct SessionActor;

pub struct SessionState {
    session: ManagedSession,
    token_last_seen: u64,
    token_cumulative: u64,
    hub: ActorRef<HubMsg>,
    supervisor: ActorRef<SessionsMsg>,
    poller: Option<ActorRef<TmuxPollerMsg>>,
    exec_path: String,
    working_timeout_ms: u64,
    working_timeout_check_interval_ms: u64,
    skip_permissions: bool,
    git_manager: GitStatusManager,
    tentacles_handle: Option<crate::tentacles::TentaclesHandle>,
}

pub struct SessionArgs {
    pub session: ManagedSession,
    pub hub: ActorRef<HubMsg>,
    pub supervisor: ActorRef<SessionsMsg>,
    pub exec_path: String,
    pub working_timeout_ms: u64,
    pub working_timeout_check_interval_ms: u64,
    /// Whether this session was created with --dangerously-skip-permissions.
    pub skip_permissions: bool,
    /// In-process tentacles server handle (if enabled).
    pub tentacles_handle: Option<crate::tentacles::TentaclesHandle>,
}


impl Actor for SessionActor {
    type Msg = SessionMsg;
    type State = SessionState;
    type Arguments = SessionArgs;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        // Spawn TmuxPoller child
        let poller_name = format!("tmux-poller-{}", &args.session.id[..8.min(args.session.id.len())]);
        let (poller_ref, _) = Actor::spawn_linked(
            Some(poller_name),
            TmuxPollerActor,
            TmuxPollerArgs {
                tmux_session: args.session.tmux_session.clone(),
                exec_path: args.exec_path.clone(),
                parent: myself.clone(),
                skip_permissions: args.skip_permissions,
            },
            myself.get_cell(),
        )
        .await?;

        // Schedule periodic checks
        myself.send_after(
            Duration::from_millis(args.working_timeout_check_interval_ms),
            || SessionMsg::CheckWorkingTimeout,
        );
        myself.send_after(Duration::from_secs(5), || SessionMsg::PollGitStatus);

        let mut git_manager = GitStatusManager::new();
        if let Some(ref cwd) = args.session.cwd {
            git_manager.track(args.session.id.clone(), cwd.clone());
        }

        Ok(SessionState {
            session: args.session,
            token_last_seen: 0,
            token_cumulative: 0,
            hub: args.hub,
            supervisor: args.supervisor,
            poller: Some(poller_ref),
            exec_path: args.exec_path,
            working_timeout_ms: args.working_timeout_ms,
            working_timeout_check_interval_ms: args.working_timeout_check_interval_ms,
            skip_permissions: args.skip_permissions,
            git_manager,
            tentacles_handle: args.tentacles_handle,
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SessionMsg::HandleEvent(event) => {
                let prev_status = state.session.status.clone();
                state.session.last_activity = now_ms();
                state.session.cwd = Some(event.cwd().to_string());

                // Track directory for git
                state
                    .git_manager
                    .track(state.session.id.clone(), event.cwd().to_string());

                match &event {
                    ClaudeEvent::PreToolUse { tool, .. } => {
                        state.session.status = SessionStatus::Working;
                        state.session.current_tool = Some(tool.clone());
                    }
                    ClaudeEvent::PostToolUse { .. } => {
                        state.session.current_tool = None;
                    }
                    ClaudeEvent::UserPromptSubmit { .. } => {
                        state.session.status = SessionStatus::Working;
                        state.session.current_tool = None;
                    }
                    ClaudeEvent::Stop { .. } | ClaudeEvent::SessionEnd { .. } => {
                        state.session.status = SessionStatus::Idle;
                        state.session.current_tool = None;
                    }
                    ClaudeEvent::PermissionRequest { tool, input, .. } => {
                        // Skip if already in Waiting state (tmux poller may have detected it first)
                        if state.session.status == SessionStatus::Waiting {
                            return Ok(());
                        }
                        state.session.status = SessionStatus::Waiting;
                        info!(
                            "Permission request for {}: {tool}",
                            state.session.name
                        );

                        // Build context from input if available
                        let context = input
                            .as_ref()
                            .and_then(|v| serde_json::to_string_pretty(v).ok())
                            .unwrap_or_default();

                        // Standard permission options
                        let options = vec![
                            crate::types::PermissionOption {
                                number: "1".into(),
                                label: "Yes".into(),
                            },
                            crate::types::PermissionOption {
                                number: "2".into(),
                                label: "Yes, allow all edits during this session".into(),
                            },
                            crate::types::PermissionOption {
                                number: "3".into(),
                                label: "No".into(),
                            },
                        ];

                        let perm_id = Uuid::new_v4().to_string();
                        state.session.pending_permission = Some(crate::types::PendingPermission {
                            id: perm_id.clone(),
                            tool: tool.clone(),
                            context: context.clone(),
                            options: options.clone(),
                        });

                        let _ = state.hub.cast(HubMsg::Broadcast(
                            crate::types::ServerMessage::PermissionPrompt {
                                session_id: state.session.id.clone(),
                                permission_id: perm_id,
                                tool: tool.clone(),
                                context,
                                options,
                            },
                        ));

                        self.notify_supervisor(state);
                    }
                    _ => {}
                }

                if state.session.status != prev_status {
                    self.notify_supervisor(state);
                }
            }

            SessionMsg::Update {
                name,
                zone_position,
            } => {
                if let Some(name) = name {
                    state.session.name = name;
                }
                if let Some(pos) = zone_position {
                    state.session.zone_position = Some(pos);
                }
                self.notify_supervisor(state);
            }

            SessionMsg::Link(claude_session_id) => {
                state.session.claude_session_id = Some(claude_session_id.clone());
                info!(
                    "Linked Claude session {} to {}",
                    &claude_session_id[..8.min(claude_session_id.len())],
                    state.session.name,
                );
                self.notify_supervisor(state);
            }

            SessionMsg::SendPrompt(text, reply) => {
                if let Some(ref poller) = state.poller {
                    if let Err(e) = poller.cast(TmuxPollerMsg::SendPrompt(text)) {
                        warn!("Failed to send prompt to poller for {}: {e}", state.session.name);
                        let _ = reply.send(Err("Tmux poller not responding".into()));
                    } else {
                        state.session.last_activity = now_ms();
                        let _ = reply.send(Ok(()));
                    }
                } else {
                    let _ = reply.send(Err("No tmux poller available".into()));
                }
            }

            SessionMsg::Cancel(reply) => {
                if let Some(ref poller) = state.poller {
                    if let Err(e) = poller.cast(TmuxPollerMsg::SendKeys("C-c".to_string())) {
                        warn!("Failed to send cancel to poller for {}: {e}", state.session.name);
                        let _ = reply.send(Err("Tmux poller not responding".into()));
                    } else {
                        let _ = reply.send(Ok(()));
                    }
                } else {
                    let _ = reply.send(Err("No tmux poller available".into()));
                }
            }

            SessionMsg::PermissionResponse(perm_id, response) => {
                // Only accept if the permission ID matches the current prompt
                let matches = state.session.pending_permission
                    .as_ref()
                    .map(|p| p.id == perm_id)
                    .unwrap_or(false);
                if !matches {
                    warn!(
                        "Ignoring stale permission response for {}: expected {:?}, got {}",
                        state.session.name,
                        state.session.pending_permission.as_ref().map(|p| &p.id),
                        perm_id,
                    );
                    return Ok(());
                }
                if let Some(ref poller) = state.poller {
                    if let Err(e) = poller.cast(TmuxPollerMsg::SendKeys(response)) {
                        warn!("Failed to send permission response to poller for {}: {e}", state.session.name);
                    } else {
                        state.session.status = SessionStatus::Working;
                        state.session.current_tool = None;
                        state.session.pending_permission = None;
                        self.notify_supervisor(state);
                    }
                }
            }

            SessionMsg::Restart(reply) => {
                let result = self.do_restart(state).await;
                match &result {
                    Ok(_session) => {
                        // Respawn the TmuxPoller
                        if let Some(ref poller) = state.poller {
                            poller.stop(Some("restarting".to_string()));
                        }
                        let poller_name = format!(
                            "tmux-poller-{}",
                            &state.session.id[..8.min(state.session.id.len())]
                        );
                        match Actor::spawn_linked(
                            Some(poller_name),
                            TmuxPollerActor,
                            TmuxPollerArgs {
                                tmux_session: state.session.tmux_session.clone(),
                                exec_path: state.exec_path.clone(),
                                parent: myself.clone(),
                                skip_permissions: state.skip_permissions,
                            },
                            myself.get_cell(),
                        )
                        .await
                        {
                            Ok((poller_ref, _)) => {
                                state.poller = Some(poller_ref);
                            }
                            Err(e) => {
                                info!("Failed to respawn TmuxPoller: {e}");
                            }
                        }
                        self.notify_supervisor(state);
                    }
                    Err(_) => {}
                }
                let _ = reply.send(result);
            }

            SessionMsg::HealthUpdate { alive } => {
                let prev_status = state.session.status.clone();
                if alive {
                    if state.session.status == SessionStatus::Offline {
                        state.session.status = SessionStatus::Idle;
                    }
                } else {
                    state.session.status = SessionStatus::Offline;
                }
                if state.session.status != prev_status {
                    self.notify_supervisor(state);
                }
            }

            SessionMsg::TokenUpdate { current } => {
                if current > state.token_last_seen {
                    let delta = current - state.token_last_seen;
                    state.token_cumulative += delta;
                    state.token_last_seen = current;

                    state.session.tokens = Some(TokenInfo {
                        current,
                        cumulative: state.token_cumulative,
                    });

                    // Broadcast token update directly via hub
                    let _ = state.hub.cast(HubMsg::Broadcast(
                        crate::types::ServerMessage::Tokens {
                            session: state.session.tmux_session.clone(),
                            current,
                            cumulative: state.token_cumulative,
                        },
                    ));

                } else if current < state.token_last_seen && current > 0 {
                    // Token count dropped — likely new conversation
                    state.token_last_seen = current;
                }
            }

            SessionMsg::PermissionDetected {
                tool,
                context,
                options,
            } => {
                // Skip if already waiting (hook event may have fired first)
                if state.session.status == SessionStatus::Waiting {
                    return Ok(());
                }
                state.session.status = SessionStatus::Waiting;
                info!(
                    "Permission prompt detected for {}: {tool}",
                    state.session.name
                );

                let perm_id = Uuid::new_v4().to_string();
                state.session.pending_permission = Some(crate::types::PendingPermission {
                    id: perm_id.clone(),
                    tool: tool.clone(),
                    context: context.clone(),
                    options: options.clone(),
                });

                let _ = state.hub.cast(HubMsg::Broadcast(
                    crate::types::ServerMessage::PermissionPrompt {
                        session_id: state.session.id.clone(),
                        permission_id: perm_id,
                        tool,
                        context,
                        options,
                    },
                ));

                self.notify_supervisor(state);
            }

            SessionMsg::PermissionResolved => {
                if state.session.status == SessionStatus::Waiting {
                    state.session.status = SessionStatus::Working;
                    state.session.current_tool = None;
                }
                state.session.pending_permission = None;
                info!("Permission prompt resolved for {}", state.session.name);

                let _ = state.hub.cast(HubMsg::Broadcast(
                    crate::types::ServerMessage::PermissionResolved {
                        session_id: state.session.id.clone(),
                    },
                ));

                self.notify_supervisor(state);
            }

            SessionMsg::CheckWorkingTimeout => {
                let now = now_ms();
                if state.session.status == SessionStatus::Working {
                    let elapsed = now.saturating_sub(state.session.last_activity);
                    if elapsed > state.working_timeout_ms {
                        info!(
                            "Session \"{}\" timed out after {}s of no activity",
                            state.session.name,
                            elapsed / 1000
                        );
                        state.session.status = SessionStatus::Idle;
                        state.session.current_tool = None;
                        self.notify_supervisor(state);
                    }
                }
                // Reschedule
                myself.send_after(
                    Duration::from_millis(state.working_timeout_check_interval_ms),
                    || SessionMsg::CheckWorkingTimeout,
                );
            }

            SessionMsg::PollGitStatus => {
                state.git_manager.poll_all().await;
                let git_status = state.git_manager.get_status(&state.session.id).cloned();
                if git_status != state.session.git_status {
                    state.session.git_status = git_status;
                    self.notify_supervisor(state);
                }
                // Reschedule
                myself.send_after(Duration::from_secs(5), || SessionMsg::PollGitStatus);
            }
        }
        Ok(())
    }

    fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: SupervisionEvent,
        state: &mut Self::State,
    ) -> impl std::future::Future<Output = Result<(), ActorProcessingErr>> + Send {
        match &msg {
            SupervisionEvent::ActorTerminated(cell, _, reason) => {
                tracing::warn!(
                    "Session {} child terminated: {} (id={}), reason: {:?}",
                    state.session.name,
                    cell.get_name().unwrap_or_default(),
                    cell.get_id(),
                    reason
                );
            }
            SupervisionEvent::ActorFailed(cell, err) => {
                tracing::error!(
                    "Session {} child failed: {} (id={}), error: {}",
                    state.session.name,
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
        state: &mut Self::State,
    ) -> impl std::future::Future<Output = Result<(), ActorProcessingErr>> + Send {
        tracing::warn!("Session actor stopped: {}", state.session.name);
        // Cancel tentacles server if running
        if let Some(handle) = state.tentacles_handle.take() {
            tracing::info!("Shutting down tentacles server on port {}", handle.port);
            handle.cancel.cancel();
        }
        async { Ok(()) }
    }
}

impl SessionActor {
    fn notify_supervisor(&self, state: &SessionState) {
        let _ = state.supervisor.cast(SessionsMsg::SessionUpdated(
            state.session.id.clone(),
            state.session.clone(),
        ));
    }

    async fn do_restart(&self, state: &mut SessionState) -> Result<ManagedSession, String> {
        let tmux_session = &state.session.tmux_session;
        let exec_path = &state.exec_path;

        // Kill existing session (ignore errors)
        let _ = tokio::process::Command::new("tmux")
            .args(["kill-session", "-t", tmux_session])
            .env("PATH", exec_path)
            .output()
            .await;

        let cwd = state.session.cwd.as_deref().unwrap_or("/tmp");
        let session_id = &state.session.id;

        // Use shell_quote to prevent command injection
        fn shell_quote(s: &str) -> String {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
        let spawn_cmd = format!(
            "VIBECRAFT_MANAGED_SESSION_ID={} PATH={} claude -c --permission-mode=bypassPermissions --dangerously-skip-permissions",
            shell_quote(session_id),
            shell_quote(exec_path),
        );

        let result = tokio::process::Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                tmux_session,
                "-c",
                cwd,
                &spawn_cmd,
            ])
            .env("PATH", exec_path)
            .output()
            .await
            .map_err(|e| format!("Failed to restart: {e}"))?;

        if !result.status.success() {
            let err = String::from_utf8_lossy(&result.stderr);
            return Err(format!("Failed to restart: {err}"));
        }

        state.session.status = SessionStatus::Idle;
        state.session.last_activity = now_ms();
        state.session.claude_session_id = None;
        state.session.current_tool = None;

        info!(
            "Restarted session: {} ({})",
            state.session.name,
            &state.session.id[..8.min(state.session.id.len())]
        );

        Ok(state.session.clone())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
