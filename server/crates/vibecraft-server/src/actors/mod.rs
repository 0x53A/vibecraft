pub mod events;
pub mod hub;
pub mod session;
pub mod sessions_supervisor;
pub mod tmux_poller;

use std::sync::Arc;

use ractor::ActorRef;
use tokio::sync::broadcast;

use crate::config::Config;

/// Shared handle passed to axum as state. All fields are cheap to clone.
#[derive(Clone)]
pub struct Actors {
    pub hub: ActorRef<hub::HubMsg>,
    pub events: ActorRef<events::EventsMsg>,
    pub sessions: ActorRef<sessions_supervisor::SessionsMsg>,
    pub config: Arc<Config>,
    /// Direct access to the broadcast channel for WS subscriptions.
    pub broadcast_tx: broadcast::Sender<String>,
}
