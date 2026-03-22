# Task 0001: Migrate Server to Actor Model (ractor)

## Status: DONE

## Problem

The Rust server used shared state (`Arc<AppState>`) with 13 `RwLock<T>` fields accessed by 6+ concurrent background tasks and HTTP handlers. This caused **deadlocks** when locks were held across `.await` points or acquired in different orders.

## Solution

Replaced `AppState` + `RwLock` with **ractor actors** + **ractor-wormhole** for ergonomic `.ask()`. Each piece of state is owned by exactly one actor. Communication is via typed messages. No shared locks = no deadlocks.

## Architecture (implemented)

```
                    ┌─────────────┐
                    │   Axum      │  HTTP + WS handlers
                    │  (routes)   │  .ask() / .cast()
                    └──────┬──────┘
                           │
         ┌─────────────────┼─────────────────┐
         ▼                 ▼                  ▼
  ┌────────────┐   ┌─────────────┐    ┌────────────┐
  │    Hub     │   │   Events    │    │  Sessions  │
  │ (broadcast)│   │  Processor  │    │ Supervisor │
  └────────────┘   └──────┬──────┘    └──────┬─────┘
         ▲                │                  │ supervises N children
         │                └──→ RouteEvent ──→┤
         │                                   │
         │           ┌───────────────────────┤
         │           ▼                       ▼
         │    ┌─────────────┐        ┌─────────────┐
         │    │  Session A  │        │  Session B  │   ...
         │    │   Actor     │        │   Actor     │
         │    └──────┬──────┘        └──────┬──────┘
         │           │ child                │
         │    ┌──────┴──────┐        ┌──────┴──────┐
         │    │ TmuxPoller  │        │ TmuxPoller  │
         │    │  (per-sess) │        │  (per-sess) │
         │    └─────────────┘        └─────────────┘
         │
    cast(Broadcast) from all actors
```

### Actors (6 types)

| Actor | File | Owns | Instances |
|-------|------|------|-----------|
| Hub | `actors/hub.rs` | broadcast channel | 1 |
| EventProcessor | `actors/events.rs` | events vec, seen_ids, pending_tool_uses | 1 |
| SessionsSupervisor | `actors/sessions_supervisor.rs` | session cache, mappings, tiles, projects | 1 |
| SessionActor | `actors/session.rs` | ManagedSession, tokens, git status | N (one per session) |
| TmuxPoller | `actors/tmux_poller.rs` | polling state, bypass_warning | N (one per session) |

### Files changed

| File | Change |
|------|--------|
| `state.rs` | **DELETED** |
| `main.rs` | Spawns actors, wires circular ref, file watcher casts to EventProcessor |
| `routes.rs` | `State<Actors>` with `.ask()` / `.cast()` instead of `Arc<AppState>` |
| `websocket.rs` | Uses `Actors`, subscribes to broadcast_tx directly |
| `actors/mod.rs` | **NEW** — module declarations, `Actors` struct |
| `actors/hub.rs` | **NEW** |
| `actors/events.rs` | **NEW** |
| `actors/session.rs` | **NEW** |
| `actors/sessions_supervisor.rs` | **NEW** |
| `actors/tmux_poller.rs` | **NEW** |
| `Cargo.toml` | Added `ractor_wormhole` git dep |

### Key design decisions

1. **One actor per session** — each SessionActor owns its ManagedSession, spawns its own TmuxPoller child
2. **Supervisor caches snapshots** — SessionsSupervisor keeps `HashMap<String, ManagedSession>` updated via `SessionUpdated` casts from children, avoiding N async roundtrips for `/sessions` GET
3. **Health check in supervisor** — single `tmux list-sessions` call, fan-out to session actors
4. **broadcast_tx exposed on Actors struct** — WS clients subscribe directly, Hub serializes and sends
5. **`.ask()` from ractor-wormhole** — clean RPC instead of ractor macros
