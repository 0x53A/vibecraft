//! Hub actor: serializes ServerMessages and broadcasts to all WebSocket clients.

use ractor::{Actor, ActorProcessingErr, ActorRef};
use tokio::sync::broadcast;
use tracing::error;

use crate::types::ServerMessage;

// ── Messages ────────────────────────────────────────────────────────────────

pub enum HubMsg {
    /// Serialize and broadcast a ServerMessage to all connected WS clients.
    Broadcast(ServerMessage),
}

// ── Actor ───────────────────────────────────────────────────────────────────

pub struct HubActor;

pub struct HubState {
    tx: broadcast::Sender<String>,
}

pub struct HubArgs {
    pub tx: broadcast::Sender<String>,
}


impl Actor for HubActor {
    type Msg = HubMsg;
    type State = HubState;
    type Arguments = HubArgs;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(HubState { tx: args.tx })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            HubMsg::Broadcast(msg) => {
                match serde_json::to_string(&msg) {
                    Ok(json) => {
                        let _ = state.tx.send(json);
                    }
                    Err(e) => {
                        error!("Failed to serialize broadcast message: {e}");
                    }
                }
            }
        }
        Ok(())
    }
}
