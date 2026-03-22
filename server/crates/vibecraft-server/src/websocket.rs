use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use ractor_wormhole::util::ActorRef_Ask;
use tokio::sync::broadcast;
use tracing::{debug, info};

use crate::actors::events::EventsMsg;
use crate::actors::sessions_supervisor::SessionsMsg;
use crate::actors::Actors;
use crate::types::{ClientMessage, ManagedSession, ServerMessage, TextTile};

const ASK_TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

/// Axum handler for WebSocket upgrade requests.
pub async fn handle_ws_upgrade(
    ws: WebSocketUpgrade,
    State(actors): State<Actors>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !is_origin_allowed(origin) {
        info!("Rejected WebSocket connection from origin: {origin}");
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }

    let origin_str = origin.to_string();
    ws.on_upgrade(move |socket| handle_ws_connection(socket, actors, origin_str))
        .into_response()
}

async fn handle_ws_connection(socket: WebSocket, actors: Actors, origin: String) {
    let client_count = actors.broadcast_tx.receiver_count() + 1;
    info!(
        "Client connected ({client_count} total){}",
        if origin.is_empty() {
            String::new()
        } else {
            format!(" from {origin}")
        }
    );

    let (mut sender, mut receiver) = socket.split();

    // ── Send initial messages ───────────────────────────────────────────

    // 1. Connected confirmation
    let last_session_id = actors
        .events
        .ask(EventsMsg::GetLastSessionId, ASK_TIMEOUT)
        .await
        .unwrap_or_else(|_| "unknown".into());

    let connected_msg = ServerMessage::Connected {
        session_id: last_session_id,
    };
    if send_server_message(&mut sender, &connected_msg).await.is_err() {
        return;
    }

    // 2. Sessions
    let sessions: Vec<ManagedSession> = actors
        .sessions
        .ask(SessionsMsg::List, ASK_TIMEOUT)
        .await
        .unwrap_or_default();
    if send_server_message(&mut sender, &ServerMessage::Sessions(sessions)).await.is_err() {
        return;
    }

    // 3. Text tiles
    let tiles: Vec<TextTile> = actors
        .sessions
        .ask(SessionsMsg::ListTiles, ASK_TIMEOUT)
        .await
        .unwrap_or_default();
    if send_server_message(&mut sender, &ServerMessage::TextTiles(tiles)).await.is_err() {
        return;
    }

    // 4. Filtered history
    let active_claude_ids = actors
        .sessions
        .ask(SessionsMsg::GetActiveClaudeIds, ASK_TIMEOUT)
        .await
        .ok();

    let history = actors
        .events
        .ask(
            |reply| EventsMsg::GetHistory {
                limit: 50,
                filter_session_ids: active_claude_ids,
                reply,
            },
            ASK_TIMEOUT,
        )
        .await
        .unwrap_or_default();

    if send_server_message(&mut sender, &ServerMessage::History(history)).await.is_err() {
        return;
    }

    // ── Spawn read + write loops ────────────────────────────────────────

    let mut rx = actors.broadcast_tx.subscribe();

    let write_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(json) => {
                    if sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!("WebSocket client lagged, skipped {n} messages");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });

    let actors_clone = actors.clone();
    let read_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_msg) => {
                            handle_client_message(&actors_clone, client_msg).await;
                        }
                        Err(e) => {
                            debug!("Failed to parse client message: {e}");
                        }
                    }
                }
                Message::Binary(_) => {
                    debug!("Received binary WebSocket message (voice not implemented)");
                }
                Message::Close(_) => {
                    break;
                }
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = write_task => {},
        _ = read_task => {},
    }

    info!(
        "Client disconnected ({} remaining)",
        actors.broadcast_tx.receiver_count().saturating_sub(1)
    );
}

async fn handle_client_message(actors: &Actors, msg: ClientMessage) {
    match msg {
        ClientMessage::Subscribe { .. } => {
            debug!("Client subscribed");
        }
        ClientMessage::GetHistory { payload } => {
            let limit = payload.and_then(|p| p.limit).unwrap_or(100);
            let history = actors
                .events
                .ask(
                    |reply| EventsMsg::GetHistory {
                        limit,
                        filter_session_ids: None,
                        reply,
                    },
                    ASK_TIMEOUT,
                )
                .await
                .unwrap_or_default();
            let _ = actors
                .hub
                .cast(crate::actors::hub::HubMsg::Broadcast(
                    ServerMessage::History(history),
                ));
            debug!("Sent {} historical events", limit);
        }
        ClientMessage::Ping => {}
        ClientMessage::VoiceStart => {
            debug!("Voice start requested (not implemented)");
        }
        ClientMessage::VoiceStop => {
            debug!("Voice stop requested (not implemented)");
        }
        ClientMessage::PermissionResponse { payload } => {
            let _ = actors.sessions.cast(SessionsMsg::SessionPermissionCast(
                payload.session_id,
                payload.permission_id,
                payload.response,
            ));
        }
    }
}

async fn send_server_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    msg: &ServerMessage,
) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|e| format!("Serialization error: {e}"))?;
    sender
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| format!("WebSocket send error: {e}"))
}

fn is_origin_allowed(origin: &str) -> bool {
    if origin.is_empty() {
        return false;
    }

    let parsed = match url::Url::parse(origin) {
        Ok(u) => u,
        Err(_) => return false,
    };

    let host = parsed.host_str().unwrap_or("");

    if host == "localhost" || host == "127.0.0.1" {
        return true;
    }

    if host == "vibecraft.sh" && parsed.scheme() == "https" {
        return true;
    }

    false
}
