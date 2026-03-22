# Vibecraft - Technical Documentation

This document explains the Vibecraft codebase for future AI assistants working on this project.

## Project Purpose

Vibecraft visualizes Claude Code's activity in real-time as a 3D workshop. When Claude uses tools (Read, Edit, Bash, etc.), a character moves to corresponding workstations in a Three.js scene. The user can also send prompts to Claude from the browser via tmux integration.

## Architecture Overview

```
Claude Code → Hook Script → POST /hook → Rust Server → WebSocket → Browser (Three.js)
                                              ↓
                                        events.jsonl
```

### Data Flow

1. **Claude Code** executes tools (Read, Edit, Bash, etc.)
2. **Hook script** (`hooks/vibecraft-hook.sh`) receives raw JSON via stdin, forwards it unmodified to the server via `POST /hook` with `X-Tmux-Session` header. **No jq** — only needs `curl`.
3. **Rust server** (`server/crates/vibecraft-server/`) converts raw hook JSON → internal `ClaudeEvent`, reads transcripts for response extraction (Stop events), writes to `~/.vibecraft/data/events.jsonl`, and broadcasts to WebSocket clients.
4. **Auto-linking**: Server matches `X-Tmux-Session` header to a managed session by tmux session name.
5. **Browser** (`src/main.ts`) receives events and moves the Claude character.

**Important:** All event processing happens server-side. The hook is a thin forwarder (~40 lines). The server is the single writer of `events.jsonl`.

### Server Startup

On startup, the server:
1. Acquires a file lock (`~/.vibecraft/data/server.lock`) — single instance only.
2. Loads persisted sessions from `sessions.json`.
3. Loads events from `events.jsonl` for history catch-up.
4. Adopts orphaned `vibecraft-*` tmux sessions not already tracked.
5. Auto-accepts "trust this folder" prompts via TmuxPoller.

### EventBus Architecture

Events are handled via a decoupled EventBus pattern in `src/events/`:

```
handleEvent(event)
    ↓
eventBus.emit(type, event, context)
    ↓
┌─────────────────────────────────────────────────────────┐
│  soundHandlers.ts      → Tool sounds, lifecycle sounds  │
│  notificationHandlers.ts → Zone floating text           │
│  characterHandlers.ts  → Movement, states               │
│  subagentHandlers.ts   → Task spawn/remove              │
│  zoneHandlers.ts       → Zone attention/status          │
│  feedHandlers.ts       → Thinking indicator             │
└─────────────────────────────────────────────────────────┘
    ↓
main.ts continues       → UI updates (DOM), special cases
```

**Design principle:** EventBus handlers update 3D scene state. main.ts handles DOM UI updates and special cases (modals).

## Key Files

### `shared/types.ts`
- `ClaudeEvent` — Union type of all event types
- `TOOL_STATION_MAP` — Maps tool names to station names (Read→bookshelf, Bash→terminal)
- `ServerMessage` / `ClientMessage` — WebSocket protocol types

**Important**: When adding new tools, update `TOOL_STATION_MAP` to assign them to stations.

### `hooks/vibecraft-hook.sh`
Minimal bash script (~40 lines). Source lives here but `npx vibecraft setup` copies it to `~/.vibecraft/hooks/vibecraft-hook.sh` (stable location that survives npm updates).

**Server-side processing** (`server/.../src/hook.rs`):
- `RawHookEvent` → `ClaudeEvent` with server-generated ID/timestamp
- Stop events: reads transcript file with retry logic (50/100/200ms) for write buffering races
- **Staleness check**: Only returns assistant response if it appears after the last user message

### Server (`server/`)
Rust (axum + tokio + ractor) WebSocket/HTTP server. See `server/CLAUDE.md` for detailed docs.

```
┌──────────┐     ┌────────────┐
│   Axum   │────▶│   Actors   │
│ handlers │     │ .ask/.cast │
└──────────┘     └─────┬──────┘
                       │
         ┌─────────────┼──────────────┐
         ▼             ▼              ▼
    ┌────────┐  ┌────────────┐  ┌──────────────────┐
    │  Hub   │  │   Events   │  │    Sessions      │
    │(bcast) │  │ Processor  │  │   Supervisor     │
    └────────┘  └────────────┘  └────────┬─────────┘
                                         │ supervises
                                    ┌────┴────┐
                                    ▼         ▼
                              SessionActor  SessionActor  ...
                                    │         │
                              TmuxPoller  TmuxPoller
```

**Key points:**
- Workspace at `server/Cargo.toml`, binary crate at `server/crates/vibecraft-server/`
- Actor files in `server/crates/vibecraft-server/src/actors/`
- Uses `ractor_wormhole` for ergonomic `.ask()` RPC
- `handle_supervisor_evt` overridden to prevent cascade kills (ractor default kills parent when child dies)
- `cast()` errors: use `let _ =` not `?` inside `handle()`, or dead target kills the actor

### `src/scene/WorkshopScene.ts`
- **World hex grid**: `createWorldHexGrid()` renders hex outlines across floor
- **Hexagonal zones**: Pointy-top hexagon platforms, honeycomb spiral layout
- 9 stations per zone (see `STATION_POSITIONS`)
- Zone positioning: axial hex coords → cartesian via `indexToHexCoord()`

### `src/entities/ClaudeMon.ts`
Robot buddy character with states: `idle`, `walking`, `working`, `thinking`.

**State machine quirk**: Don't set idle state while walking, or the character stops mid-path. Check `state !== 'walking'` before setting idle.

Animation system in `src/entities/`: `AnimationTypes.ts`, `IdleBehaviors.ts`, `WorkingBehaviors.ts`. Dev panel: `Alt+D`.

### Other UI modules
- `src/entities/SubagentManager.ts` — Mini-Claudes at portal for Task tools (60% scale, fan pattern)
- `src/scene/ZoneNotifications.ts` — Floating tool completion text above zones
- `src/events/EventClient.ts` — WebSocket client with auto-reconnect. History handler pre-scans for `post_tool_use` to prevent stale "pending" icons.
- `src/ui/FeedManager.ts` — Activity feed with session filtering, thinking indicator
- `src/ui/QuestionModal.ts` — AskUserQuestion UI with option buttons
- `src/ui/PermissionModal.ts` — Tool permission UI (number key shortcuts, no escape close)
- `src/ui/DrawMode.ts` — Hex painting mode (`D` key), 6 colors, brush sizes, 3D stacking
- `src/ui/TextLabelModal.ts` — Themed textarea replacing browser `prompt()`

## Event Types

| Type | When | Key Fields |
|------|------|------------|
| `pre_tool_use` | Before tool executes | `tool`, `toolUseId`, `input` |
| `post_tool_use` | After tool completes | `tool`, `toolUseId`, `success`, `duration` |
| `stop` | Claude stops responding | `reason` |
| `user_prompt_submit` | User sends prompt | `prompt` |
| `notification` | System notification | `message` |

## Common Tasks

### Adding a new station
1. Add position to `STATION_POSITIONS` in `WorkshopScene.ts`
2. Create station mesh in `createStations()`
3. Update `StationType` in `shared/types.ts`
4. Map relevant tools in `TOOL_STATION_MAP`

### Debugging events
1. Check `~/.vibecraft/data/events.jsonl` for raw events
2. `RUST_LOG=debug` for server logs
3. Browser console shows event client logs

## State Management

State is distributed across ractor actors — each owns its state exclusively (no shared locks).

| Actor | Owns | Instances |
|-------|------|-----------|
| **Hub** | broadcast channel | 1 |
| **EventProcessor** | events vec, seen_ids, pending_tool_uses | 1 |
| **SessionsSupervisor** | session cache, claude→managed map, tiles, projects, tentacles registries | 1 |
| **SessionActor** | `ManagedSession`, token tracking, git status, tentacles handle | 1 per session |
| **TmuxPoller** | tmux polling state, bypass warning tracking | 1 per session |

### Session Status Transitions

| Event | Status Change |
|-------|---------------|
| `user_prompt_submit` | → `working` |
| `pre_tool_use` | → `working` |
| `stop` / `session_end` | → `idle` |
| tmux session dies | → `offline` |
| No activity for 2 min | → `idle` |

### Session Auto-Linking

When vibecraft creates a tmux session, it sets `VIBECRAFT_MANAGED_SESSION_ID` env var. The hook reads this and POSTs to `/sessions/{id}/link`. Also matches by `X-Tmux-Session` header for adopted/restarted sessions.

### Persistence

Sessions persisted to `sessions.json` by SessionsSupervisor (saved on create/update/delete/link/status change). Client rebuilds its local map from server data on every `sessions` update — server is authoritative.

### Health Checks

- **Session health** (every 5s): Supervisor runs `tmux list-sessions`, fans out `HealthUpdate`
- **Working timeout**: SessionActor self-schedules, transitions stale `working` → `idle`
- **Git status** (every 5s): Each SessionActor polls git status for its directory

## Tentacles (Remote Execution Proxy)

Routes Claude's file/shell tools through an MCP server dispatching to configurable targets. Embedded in vibecraft server process.

| Type | Execution |
|------|-----------|
| **host** | Direct `bash -c` on host |
| **container** | `docker exec` (existing, from image, or from Dockerfile) |
| **ssh** | Not yet implemented (stub) |

Key files in `server/.../src/tentacles/`: `mod.rs` (MCP server + management API), `targets.rs` (trait + registry), `host.rs`, `docker.rs`, `ssh.rs`.

Runtime target management via API: `GET/POST/DELETE /api/sessions/{id}/targets`.

Tentacles MCP tools are additive — always available alongside built-in tools. `--tools` only affects built-ins.

## Sound System

See **[docs/SOUND.md](docs/SOUND.md)** for full catalog and spatial audio docs.

Programmatic synthesis via **Tone.js** — no audio files. Spatial audio provides distance-based volume and stereo panning relative to camera. Sounds triggered via EventBus handlers in `src/events/handlers/soundHandlers.ts`.

## Configuration

### Central Defaults

All defaults in **`shared/defaults.ts`** (single source of truth). Imported by server and vite config. Frontend gets ports via Vite's `define` at build time.

### Data Directory (`~/.vibecraft/data/`)

`events.jsonl`, `sessions.json`, `tiles.json`, `pending-prompt.txt`, `server.lock`

### Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `VIBECRAFT_PORT` | 4003 | Server port |
| `VIBECRAFT_CLIENT_PORT` | 4002 | Vite dev server port |
| `VIBECRAFT_DEBUG` | false | Verbose logging |
| `DEEPGRAM_API_KEY` | (none) | Voice input |

See `shared/defaults.ts` and `.env` for full list.

## Keyboard Shortcuts

| Key | Context | Action |
|-----|---------|--------|
| `Tab`/`Esc` | Anywhere | Toggle Workshop ↔ Activity Feed |
| `1-6` | Not in input | Switch to session 1-6 |
| `Alt+key` | Anywhere | Switch to session (works in inputs) |
| `0` / `` ` `` | Not in input | All sessions / overview |
| `Alt+N` | Anywhere | New session modal |
| `Alt+A` | Anywhere | Next session needing attention |
| `Alt+Space` | Anywhere | Expand most recent "show more" |
| `Alt+R` | Anywhere | Toggle voice recording |
| `F` | Not in input | Toggle follow-active mode |
| `P` | Not in input | Toggle station panels |
| `D` | Not in input | Toggle draw mode |
| `Ctrl+C` | Not in input | Copy if selected, else interrupt session |

Extended session keybinds: `Q-Y` (7-12), `A-H` (13-18), `Z-N` (19-24).

## Build & CLI

| Script | Description |
|--------|-------------|
| `pnpm run dev` | Start dev server (Vite + cargo watch) |
| `pnpm run dev:client` | Vite dev server only |
| `pnpm run dev:server` | Rust server with cargo watch |
| `pnpm run build` | Build both server and client |

```bash
vibecraft                 # Start server
vibecraft setup           # Install hooks to ~/.vibecraft/hooks/, configure in ~/.claude/settings.json
vibecraft --port 4000     # Custom port
```

## Future Work

- [ ] **SSH target**: Implement SSH execution backend for tentacles
- [ ] **Target access control**: Lock targets externally (e.g., container-only overnight)
- [ ] **Session replay**: Replay events from events.jsonl
- [ ] **File system map**: 3D visualization of touched files
- [ ] **VR support**: WebXR for immersive view
