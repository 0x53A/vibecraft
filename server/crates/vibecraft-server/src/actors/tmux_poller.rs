//! TmuxPoller actor: per-session background polling for tokens and permissions.
//!
//! Each managed session spawns a TmuxPoller as a child actor. The poller
//! periodically captures the tmux pane and parses token counts / permission
//! prompts, sending results back to the parent SessionActor.

use std::time::Duration;

use ractor::{Actor, ActorProcessingErr, ActorRef};
use tracing::info;

use crate::tmux;

use super::session::SessionMsg;

// ── Messages ────────────────────────────────────────────────────────────────

pub enum TmuxPollerMsg {
    /// Self-scheduled: poll tokens + permissions.
    Poll,
    /// Send a prompt to this session's tmux pane.
    SendPrompt(String),
    /// Send keys (e.g. Ctrl+C, permission response digit).
    SendKeys(String),
}

// ── Actor ───────────────────────────────────────────────────────────────────

pub struct TmuxPollerActor;

pub struct TmuxPollerState {
    tmux_session: String,
    exec_path: String,
    parent: ActorRef<SessionMsg>,
    bypass_warning_handled: bool,
    had_permission: bool,
    /// Whether this session was created with --dangerously-skip-permissions.
    /// Only auto-accept bypass warnings for these sessions.
    skip_permissions: bool,
}

pub struct TmuxPollerArgs {
    pub tmux_session: String,
    pub exec_path: String,
    pub parent: ActorRef<SessionMsg>,
    pub skip_permissions: bool,
}


impl Actor for TmuxPollerActor {
    type Msg = TmuxPollerMsg;
    type State = TmuxPollerState;
    type Arguments = TmuxPollerArgs;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        // Schedule first poll after a short delay
        myself.send_after(Duration::from_secs(1), || TmuxPollerMsg::Poll);
        Ok(TmuxPollerState {
            tmux_session: args.tmux_session,
            exec_path: args.exec_path,
            parent: args.parent,
            bypass_warning_handled: false,
            had_permission: false,
            skip_permissions: args.skip_permissions,
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            TmuxPollerMsg::Poll => {
                self.do_poll(state).await;
                // Schedule next poll
                myself.send_after(Duration::from_secs(2), || TmuxPollerMsg::Poll);
            }

            TmuxPollerMsg::SendPrompt(text) => {
                if let Err(e) =
                    tmux::send_to_tmux_safe(&state.tmux_session, &text, &state.exec_path).await
                {
                    info!("Failed to send prompt to {}: {e}", state.tmux_session);
                }
            }

            TmuxPollerMsg::SendKeys(keys) => {
                let _ = tmux::exec_with_path(
                    "tmux",
                    &["send-keys", "-t", &state.tmux_session, &keys],
                    &state.exec_path,
                )
                .await;
            }

        }
        Ok(())
    }
}

impl TmuxPollerActor {
    async fn capture_pane(&self, state: &TmuxPollerState, lines: u32) -> Option<String> {
        if tmux::validate_tmux_session(&state.tmux_session).is_err() {
            return None;
        }
        let out = tmux::exec_with_path(
            "tmux",
            &[
                "capture-pane",
                "-t",
                &state.tmux_session,
                "-p",
                "-S",
                &format!("-{lines}"),
            ],
            &state.exec_path,
        )
        .await
        .ok()?;

        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            None
        }
    }

    async fn do_poll(&self, state: &mut TmuxPollerState) {
        if tmux::validate_tmux_session(&state.tmux_session).is_err() {
            return;
        }

        let output = match self.capture_pane(state, 50).await {
            Some(o) => o,
            None => return,
        };

        // Parse tokens
        if let Some(tokens) = tmux::parse_tokens_from_output(&output) {
            let _ = state
                .parent
                .cast(SessionMsg::TokenUpdate { current: tokens });
        }

        // Auto-accept "trust this folder" prompt
        if tmux::detect_trust_prompt(&output) {
            // Re-capture to confirm prompt is still on screen
            if let Some(fresh) = self.capture_pane(state, 15).await {
                if tmux::detect_trust_prompt(&fresh) {
                    info!(
                        "Trust folder prompt detected for {}, auto-accepting...",
                        state.tmux_session
                    );
                    let _ = tmux::exec_with_path(
                        "tmux",
                        &["send-keys", "-t", &state.tmux_session, "Enter"],
                        &state.exec_path,
                    )
                    .await;
                    return;
                }
            }
        }

        // Check bypass warning — only auto-accept if this session uses --dangerously-skip-permissions
        if state.skip_permissions && tmux::detect_bypass_warning(&output) && !state.bypass_warning_handled {
            // Re-capture to confirm warning is still on screen (avoid race with user interaction)
            if let Some(fresh) = self.capture_pane(state, 15).await {
                if tmux::detect_bypass_warning(&fresh) {
                    info!(
                        "Bypass permissions warning detected for {}, auto-accepting...",
                        state.tmux_session
                    );
                    state.bypass_warning_handled = true;
                    let _ = tmux::exec_with_path(
                        "tmux",
                        &["send-keys", "-t", &state.tmux_session, "2"],
                        &state.exec_path,
                    )
                    .await;
                    return;
                }
            }
        }

        // Detect permission prompts
        let prompt = tmux::detect_permission_prompt(&output);

        if let Some((tool, context, options)) = prompt {
            if !state.had_permission {
                state.had_permission = true;
                let _ = state.parent.cast(SessionMsg::PermissionDetected {
                    tool,
                    context,
                    options,
                });
            }
        } else if state.had_permission {
            state.had_permission = false;
            let _ = state.parent.cast(SessionMsg::PermissionResolved);
        }
    }
}
