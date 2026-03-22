# Vibecraft Server (Rust)

Rust rewrite of the original TypeScript server (`server_legacy/`). Uses axum + tokio + ractor actors.

## Architecture: Actor Model

The server uses **ractor** actors for all state management. No shared locks, no deadlocks.

```
┌──────────┐   ┌────────────┐   ┌─────────────┐
│   Axum   │──▶│   Actors   │   │ File Watcher│
│ handlers │   │  .ask()    │   │  (tokio)    │
└──────────┘   │  .cast()   │   └──────┬──────┘
               └─────┬──────┘          │
                     │          cast(Ingest)
    ┌────────────────┼────────────────┐
    ▼                ▼                ▼
┌────────┐   ┌────────────┐   ┌──────────────┐
│  Hub   │   │   Events   │   │  Sessions    │
│(bcast) │   │ Processor  │   │  Supervisor  │
└────────┘   └────────────┘   └───────┬──────┘
    ▲                                 │
    │                    ┌────────────┤
    │                    ▼            ▼
    │             ┌───────────┐ ┌───────────┐
    │             │ Session A │ │ Session B │ ...
    │             └─────┬─────┘ └─────┬─────┘
    │                   │             │
    │             ┌─────┴─────┐ ┌─────┴─────┐
    │             │TmuxPoller │ │TmuxPoller │
    │             └───────────┘ └───────────┘
    │
  cast(Broadcast) from all actors
```

### Actors

| Actor | File | Purpose |
|-------|------|---------|
| **Hub** | `actors/hub.rs` | Serializes `ServerMessage` → JSON, sends on broadcast channel |
| **EventProcessor** | `actors/events.rs` | Dedup, duration calc, stores events, routes to sessions |
| **SessionsSupervisor** | `actors/sessions_supervisor.rs` | Manages session collection, tiles, projects, persistence |
| **SessionActor** | `actors/session.rs` | Per-session state, status transitions, git polling |
| **TmuxPoller** | `actors/tmux_poller.rs` | Per-session tmux polling (tokens, permissions) |

### Message flow

**Hook event → 3D scene:**
1. Hook POSTs raw Claude Code JSON to `POST /hook` with `X-Tmux-Session` header
2. Route handler converts raw → `ClaudeEvent` via `hook.rs` (includes transcript reading for Stop/PreToolUse)
3. Route writes converted event to `events.jsonl`, casts `EventsMsg::Ingest`
4. Route sends `SessionsMsg::LinkByTmux` to auto-link by tmux session name
5. EventProcessor deduplicates, calculates duration, stores
6. Broadcasts via `Hub::Broadcast(Event(...))`
7. Casts `SessionsMsg::RouteEvent` → supervisor looks up `claude_to_managed` map → finds SessionActor
8. SessionActor updates status, casts `SessionUpdated` back to supervisor
9. Supervisor updates cache, broadcasts sessions list via Hub, persists to disk

**Session auto-linking:**
1. Claude Code fires a hook → hook script detects tmux session name via `tmux display-message`
2. Hook POSTs to `/hook` with `X-Tmux-Session: vibecraft-XXXX` header
3. Server receives `LinkByTmux(tmux_name, claude_session_id)` → finds managed session by tmux name
4. Maps `claude_session_id → managed_session_id` — all future events route correctly
5. Works for newly spawned sessions, adopted orphans, and sessions surviving server restarts

**HTTP request/reply:** Routes use `.ask()` (from ractor-wormhole) for RPC-style calls.

### Important: Supervision

ractor's **default `handle_supervisor_evt` kills the parent** when any linked child dies. All actors override this to log and return `Ok(())`. Without this, stopping a SessionActor (e.g., on delete) cascade-kills the supervisor, breaking all subsequent requests.

Similarly, **never use `?` on `cast()` calls** inside `handle()` — if the target actor is dead, the `Err` propagates and kills the calling actor. Use `let _ = actor_ref.cast(...)` instead.

## Project Structure

```
server/
├── Cargo.toml                          # Workspace root
└── crates/
    └── vibecraft-server/
        ├── Cargo.toml
        └── src/
            ├── main.rs                 # Entry point, spawns actors, mutex, tmux adoption
            ├── config.rs               # Config from env vars with defaults
            ├── hook.rs                 # Raw hook event parsing, transcript reading
            ├── types.rs                # All types (events, sessions, messages, tiles)
            ├── routes.rs               # Axum HTTP handlers (/hook, /event, /sessions, etc.)
            ├── websocket.rs            # WebSocket upgrade + read/write loops
            ├── tmux.rs                 # tmux interaction (exec, parsing, detection)
            ├── git.rs                  # GitStatusManager (polls git status)
            ├── projects.rs             # ProjectsManager (known dirs for autocomplete)
            ├── actors/
            │   ├── mod.rs              # Actors struct, module declarations
            │   ├── hub.rs              # Hub actor (broadcast)
            │   ├── events.rs           # EventProcessor actor
            │   ├── session.rs          # SessionActor (one per session)
            │   ├── sessions_supervisor.rs # SessionsSupervisor actor
            │   └── tmux_poller.rs      # TmuxPoller actor (one per session)
            └── tentacles/              # MCP tool proxy (embedded)
                ├── mod.rs              # MCP server, management API, start()
                ├── targets.rs          # Target trait, TargetRegistry
                ├── host.rs             # HostTarget (direct execution)
                ├── docker.rs           # DockerTarget (docker exec, create from image/dockerfile)
                └── ssh.rs              # SshTarget (stub)
```

## Dependencies

Key crates: `axum`, `tokio`, `ractor`, `ractor_wormhole` (git), `serde`/`serde_json`, `notify`, `uuid`, `tracing`, `tower-http`, `rmcp` (MCP server), `schemars`.

## Building

```sh
cd server && cargo build -p vibecraft-server          # debug
cd server && cargo build -p vibecraft-server --release # release
```

Or via pnpm from the repo root:
```sh
pnpm run dev:server    # cargo watch (auto-rebuild)
pnpm run build:server  # release build
```

## Tentacles (Embedded MCP Tool Proxy)

Tentacles is an MCP server that proxies Claude's file/shell tools to configurable execution targets. It runs in-process (not a separate binary) — one HTTP server per session on a random port.

### How it works

1. **Session creation** (`sessions_supervisor.rs`): If `SessionFlags.tentacles.enabled`, calls `tentacles::start(&targets)`.
2. **`tentacles::start()`** (`tentacles/mod.rs`): Builds a `TargetRegistry`, starts an axum HTTP server on `127.0.0.1:0`, returns `TentaclesHandle { port, registry, cancel }`.
3. **MCP config**: Written to `~/.vibecraft/data/mcp/{id}.json` with `{ "mcpServers": { "tentacles": { "url": "http://127.0.0.1:<port>/mcp" } } }`.
4. **Tool restriction**: User's tool selections from the UI apply to built-in tools via `--tools`. MCP tools (`mcp__tentacles__*`) are always available regardless of `--tools`.
5. **Cleanup**: `SessionActor::post_stop()` calls `handle.cancel.cancel()` to shut down the HTTP server.

### Target management

The `TargetRegistry` is stored in `SupervisorState.tentacles_registries` keyed by session ID. Route handlers access it directly via `SessionsMsg::GetTentaclesRegistry` — no HTTP proxy needed.

### Adding a new target type

1. Create `tentacles/my_target.rs` implementing `Target` trait (async `exec` + `exec_with_stdin`)
2. Add the type to `parse_target_json()` in `tentacles/mod.rs`
3. Add UI option in `index.html` tentacles section

## Legacy TS Server

The original TypeScript server is preserved at `server_legacy/` for reference.
