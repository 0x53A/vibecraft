mod actors;
mod config;
#[allow(dead_code)]
mod git;
mod hook;
#[allow(dead_code)]
mod projects;
mod routes;
pub mod tentacles;
#[allow(dead_code)]
mod tmux;
#[allow(dead_code)]
mod types;
mod websocket;

use std::sync::Arc;

use ractor::Actor;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

use crate::actors::hub::{HubActor, HubArgs};
use crate::actors::events::{EventsActor, EventsArgs, EventsMsg};
use crate::actors::sessions_supervisor::{SessionsSupervisorActor, SessionsMsg, SupervisorArgs};
use crate::actors::Actors;
use crate::config::Config;

/// Tmux session name prefix for vibecraft-managed sessions.
const TMUX_PREFIX: &str = "vibecraft-";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| {
                if std::env::var("VIBECRAFT_DEBUG").as_deref() == Ok("true") {
                    EnvFilter::new("debug")
                } else {
                    EnvFilter::new("info")
                }
            }),
        )
        .init();

    let config = Config::from_env();
    let port = config.port;

    // ── Mutex: ensure only one server instance ──────────────────────────
    let lock_path = config::expand_home("~/.vibecraft/data/server.lock");
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)?;
    use std::os::unix::io::AsRawFd;
    let locked = unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    if locked != 0 {
        eprintln!("Another vibecraft server is already running (lock: {})", lock_path.display());
        std::process::exit(1);
    }
    // Write PID so it's easy to identify the holder
    use std::io::Write;
    let mut lock = lock_file;
    let _ = write!(lock, "{}", std::process::id());

    tracing::info!("Starting Vibecraft server (actor model)...");

    // Create broadcast channel (shared between Hub and WS subscriptions)
    let (broadcast_tx, _) = broadcast::channel::<String>(1024);

    // ── Spawn actors ────────────────────────────────────────────────────

    // 1. Hub
    let (hub_ref, _) = Actor::spawn(
        Some("hub".to_string()),
        HubActor,
        HubArgs {
            tx: broadcast_tx.clone(),
        },
    )
    .await?;

    // 2. EventProcessor
    let (events_ref, _) = Actor::spawn(
        Some("events".to_string()),
        EventsActor,
        EventsArgs {
            max_events: config.max_events,
            hub: hub_ref.clone(),
        },
    )
    .await?;

    // 3. SessionsSupervisor
    let (sessions_ref, _) = Actor::spawn(
        Some("sessions".to_string()),
        SessionsSupervisorActor,
        SupervisorArgs {
            hub: hub_ref.clone(),
            config: config.clone(),
        },
    )
    .await?;

    // Wire up the circular reference: EventProcessor needs SessionsSupervisor
    events_ref.cast(EventsMsg::SetSessions(sessions_ref.clone()))?;

    // Load events from file (catch up on events from before this server started)
    actors::events::load_events_from_file(&events_ref, &config.events_file).await;

    // ── Adopt orphaned tmux sessions ────────────────────────────────────
    // Find any vibecraft-* tmux sessions that aren't tracked by a managed
    // session and adopt them so they show up in the UI immediately.
    adopt_orphan_tmux_sessions(&sessions_ref, &config).await;

    // Build the Actors handle
    let actors = Actors {
        hub: hub_ref,
        events: events_ref,
        sessions: sessions_ref,
        config: Arc::new(config),
        broadcast_tx,
    };

    // Build router and start server
    let app = routes::router(actors.clone());

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    tracing::info!("Server running on http://localhost:{port}");
    tracing::info!("  WebSocket: ws://localhost:{port}/ws");
    tracing::info!("  API: http://localhost:{port}/health, /sessions, /stats, /event");

    axum::serve(listener, app).await?;

    Ok(())
}

/// Discover vibecraft-* tmux sessions and adopt any that aren't already tracked.
async fn adopt_orphan_tmux_sessions(
    sessions_ref: &ractor::ActorRef<SessionsMsg>,
    config: &Config,
) {
    use ractor_wormhole::util::ActorRef_Ask;

    // List all tmux sessions
    let result = tokio::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}:#{pane_current_path}"])
        .env("PATH", &config.exec_path)
        .output()
        .await;

    let tmux_sessions: Vec<(String, String)> = match result {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .lines()
                .filter_map(|line| {
                    let (name, path) = line.split_once(':')?;
                    if name.starts_with(TMUX_PREFIX) {
                        Some((name.to_string(), path.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        }
        _ => return,
    };

    if tmux_sessions.is_empty() {
        return;
    }

    // Get currently tracked sessions
    let known: Vec<types::ManagedSession> = sessions_ref
        .ask(SessionsMsg::List, Some(std::time::Duration::from_secs(5)))
        .await
        .unwrap_or_default();

    let known_tmux_names: std::collections::HashSet<&str> = known
        .iter()
        .map(|s| s.tmux_session.as_str())
        .collect();

    for (tmux_name, cwd) in &tmux_sessions {
        if known_tmux_names.contains(tmux_name.as_str()) {
            continue;
        }

        tracing::info!("Adopting orphaned tmux session: {tmux_name} (cwd: {cwd})");

        // Create a managed session for it (without spawning a new tmux session)
        let result = sessions_ref
            .ask(
                |reply| SessionsMsg::AdoptTmuxSession {
                    tmux_session: tmux_name.clone(),
                    cwd: cwd.clone(),
                    reply,
                },
                Some(std::time::Duration::from_secs(5)),
            )
            .await;

        match result {
            Ok(Ok(session)) => {
                tracing::info!("Adopted session: {} ({})", session.name, &session.id[..8]);
            }
            Ok(Err(e)) => {
                tracing::warn!("Failed to adopt {tmux_name}: {e}");
            }
            Err(e) => {
                tracing::warn!("Failed to adopt {tmux_name}: {e}");
            }
        }
    }
}

