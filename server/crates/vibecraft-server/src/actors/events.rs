//! EventProcessor actor: owns events, deduplication, duration calculation, stats.

use std::collections::{HashMap, HashSet};

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use tracing::debug;

use crate::types::{ClaudeEvent, ServerMessage};

use super::hub::HubMsg;
use super::sessions_supervisor::SessionsMsg;

// ── Messages ────────────────────────────────────────────────────────────────

pub enum EventsMsg {
    /// Ingest a new event (from file watcher or HTTP POST).
    Ingest(ClaudeEvent),
    /// Get recent history, optionally filtered to specific claude session IDs.
    GetHistory {
        limit: usize,
        filter_session_ids: Option<HashSet<String>>,
        reply: RpcReplyPort<Vec<ClaudeEvent>>,
    },
    /// Get stats (tool counts, durations).
    GetStats(RpcReplyPort<StatsResponse>),
    /// Get total event count.
    GetEventCount(RpcReplyPort<usize>),
    /// Get last session ID (for WS connected message).
    GetLastSessionId(RpcReplyPort<String>),
    /// Wire up the sessions supervisor reference (called once at startup).
    SetSessions(ActorRef<SessionsMsg>),
}

#[derive(Debug, Clone)]
pub struct StatsResponse {
    pub total_events: usize,
    pub tool_counts: HashMap<String, u64>,
    pub avg_durations: HashMap<String, u64>,
}

// ── Actor ───────────────────────────────────────────────────────────────────

pub struct EventsActor;

pub struct EventsState {
    events: Vec<ClaudeEvent>,
    seen_ids: HashSet<String>,
    pending_tool_uses: HashMap<String, ClaudeEvent>,
    max_events: usize,
    hub: ActorRef<HubMsg>,
    sessions: Option<ActorRef<SessionsMsg>>,
}

pub struct EventsArgs {
    pub max_events: usize,
    pub hub: ActorRef<HubMsg>,
}



impl Actor for EventsActor {
    type Msg = EventsMsg;
    type State = EventsState;
    type Arguments = EventsArgs;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(EventsState {
            events: Vec::new(),
            seen_ids: HashSet::new(),
            pending_tool_uses: HashMap::new(),
            max_events: args.max_events,
            hub: args.hub,
            sessions: None,
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            EventsMsg::Ingest(mut event) => {
                // 1. Dedup
                if state.seen_ids.contains(event.id()) {
                    debug!("Skipping duplicate event: {}", event.id());
                    return Ok(());
                }
                state.seen_ids.insert(event.id().to_string());

                // Trim seen_ids to prevent unbounded growth
                if state.seen_ids.len() > state.max_events * 2 {
                    let ids: Vec<String> = state.seen_ids.iter().cloned().collect();
                    state.seen_ids.clear();
                    for id in ids.into_iter().rev().take(state.max_events) {
                        state.seen_ids.insert(id);
                    }
                }

                // 2. Match pre/post tool_use for duration calculation
                match &event {
                    ClaudeEvent::PreToolUse { tool_use_id, .. } => {
                        state
                            .pending_tool_uses
                            .insert(tool_use_id.clone(), event.clone());
                    }
                    ClaudeEvent::PostToolUse {
                        tool_use_id,
                        timestamp,
                        ..
                    } => {
                        if let Some(pre) = state.pending_tool_uses.remove(tool_use_id) {
                            let dur = timestamp.saturating_sub(pre.timestamp());
                            if let ClaudeEvent::PostToolUse {
                                ref mut duration, ..
                            } = event
                            {
                                *duration = Some(dur);
                            }
                        }
                    }
                    _ => {}
                }

                // 3. Store, trim
                state.events.push(event.clone());
                if state.events.len() > state.max_events {
                    let drain_count = state.events.len() - state.max_events;
                    state.events.drain(..drain_count);
                }

                // 4. Broadcast event
                let _ = state
                    .hub
                    .cast(HubMsg::Broadcast(ServerMessage::Event(event.clone())));

                // 5. Route to sessions supervisor for status updates
                if let Some(ref sessions) = state.sessions {
                    let _ = sessions.cast(SessionsMsg::RouteEvent(event));
                }
            }

            EventsMsg::GetHistory {
                limit,
                filter_session_ids,
                reply,
            } => {
                let history: Vec<ClaudeEvent> = if let Some(ids) = filter_session_ids {
                    state
                        .events
                        .iter()
                        .filter(|e| ids.contains(e.session_id()))
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .take(limit)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect()
                } else {
                    state
                        .events
                        .iter()
                        .rev()
                        .take(limit)
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect()
                };
                let _ = reply.send(history);
            }

            EventsMsg::GetStats(reply) => {
                let mut tool_counts: HashMap<String, u64> = HashMap::new();
                let mut tool_durations: HashMap<String, Vec<u64>> = HashMap::new();

                for event in &state.events {
                    if let ClaudeEvent::PostToolUse {
                        tool, duration, ..
                    } = event
                    {
                        *tool_counts.entry(tool.clone()).or_default() += 1;
                        if let Some(dur) = duration {
                            tool_durations.entry(tool.clone()).or_default().push(*dur);
                        }
                    }
                }

                let avg_durations: HashMap<String, u64> = tool_durations
                    .into_iter()
                    .map(|(tool, durs)| {
                        let avg = durs.iter().sum::<u64>() / durs.len().max(1) as u64;
                        (tool, avg)
                    })
                    .collect();

                let _ = reply.send(StatsResponse {
                    total_events: state.events.len(),
                    tool_counts,
                    avg_durations,
                });
            }

            EventsMsg::GetEventCount(reply) => {
                let _ = reply.send(state.events.len());
            }

            EventsMsg::GetLastSessionId(reply) => {
                let id = state
                    .events
                    .last()
                    .map(|e| e.session_id().to_string())
                    .unwrap_or_else(|| "unknown".into());
                let _ = reply.send(id);
            }

            EventsMsg::SetSessions(sessions_ref) => {
                state.sessions = Some(sessions_ref);
            }
        }
        Ok(())
    }

    fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        _state: &mut Self::State,
    ) -> impl std::future::Future<Output = Result<(), ActorProcessingErr>> + Send {
        tracing::error!("EventProcessor actor stopped!");
        async { Ok(()) }
    }
}

/// Load events from a JSONL file directly into an EventProcessor actor.
/// Called once at startup before the actor processes live messages.
pub async fn load_events_from_file(
    events_ref: &ActorRef<EventsMsg>,
    path: &std::path::Path,
) {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => {
            debug!("Events file not found: {}", path.display());
            return;
        }
    };

    let lines: Vec<&str> = content.trim().split('\n').filter(|l| !l.is_empty()).collect();
    let mut count = 0;
    for line in &lines {
        match serde_json::from_str::<ClaudeEvent>(line) {
            Ok(event) => {
                let _ = events_ref.cast(EventsMsg::Ingest(event));
                count += 1;
            }
            Err(e) => {
                debug!("Failed to parse event line: {e}");
            }
        }
    }

    tracing::info!("Loaded {count} events from file");
}
