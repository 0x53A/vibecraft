# Vision: Vibecraft as a Claude Code Control Plane

Vibecraft started as a 3D visualizer for Claude Code activity. The vision is to evolve it into a **full control plane** for managing Claude Code instances — spawning, monitoring, controlling tool execution, and restricting access.

## Core idea

Claude Code instances should be able to operate in sandboxed environments (Docker containers, remote hosts via SSH) with all tool execution proxied through a layer we control. The user can:

- Spawn Claude Code sessions from the UI
- Choose where each session's tools execute (host, container, remote)
- Switch targets at runtime
- Lock down targets (e.g., container-only overnight) so Claude can't escape
- Monitor everything from a web dashboard

## Architecture (target)

```
┌──────────────────────────────────────────────────┐
│  Vibecraft Control Plane (Rust server)           │
│                                                  │
│  ┌────────────┐  ┌────────────┐  ┌───────────┐  │
│  │  Session    │  │  MCP Server│  │  Web UI   │  │
│  │  Manager    │  │  per session│  │  (3D viz) │  │
│  │  (tmux)     │  │  (HTTP)    │  │           │  │
│  └─────┬──────┘  └──────┬─────┘  └───────────┘  │
│        │                │                         │
│        │  spawns        │  proxies tools           │
│        ▼                ▼                         │
│  ┌──────────┐    ┌──────────────┐                │
│  │ claude   │───▶│  Exec Target │                │
│  │ --mcp-   │    │  • container │                │
│  │  config   │    │  • host     │                │
│  └──────────┘    │  • SSH      │                │
│                  └──────────────┘                │
└──────────────────────────────────────────────────┘
```

## MCP tool proxying

Each Claude Code session connects to its own MCP server (HTTP transport, unique port per session). The MCP server exposes the standard Claude Code tools (Bash, Read, Write, Edit, Glob, Grep) but executes them against a configurable target.

**Key design decision:** every tool takes `target` as a required first parameter (e.g., `Bash(target: "container", command: "ls")`). This is stateless — no "current target" to manage, no state bugs. Claude explicitly says where to run each command.

**Access control:** managed externally (control file, CLI, web UI), not by Claude. Before bed, lock to container-only. Claude's MCP tools check allowed targets and refuse if locked out.

### Implementation

Tentacles is embedded in the vibecraft server at `server/crates/vibecraft-server/src/tentacles/`. The original standalone prototype at `~/src/tentacles/` still exists for reference/standalone testing.

The server uses HTTP transport (rmcp streamable HTTP) — each session gets its own MCP server on a random port. The server is started in-process (tokio task), not as a child process.

## Session spawning with MCP

When vibecraft creates a session with tentacles enabled:
1. Call `tentacles::start(targets)` in-process — binds random port, returns handle
2. Write `--mcp-config` JSON with `{ "tentacles": { "url": "http://127.0.0.1:<port>/mcp" } }`
3. Spawn Claude Code with `--tools "WebSearch,WebFetch,Agent,Task,..." --disallowed-tools LSP --mcp-config <path>`
4. Built-in file/shell tools disabled; Claude uses `mcp__tentacles__Bash`, etc. MCP tools are always available — `--tools` only controls built-ins.

Resuming sessions: `--resume <session-id>` with a new `--mcp-config` (port may change).

## Target types

- **host** — direct execution on the host machine (working)
- **container** — `docker exec <container> ...` (working, with auto-create from image/Dockerfile)
- **ssh** — `ssh <host> ...` for remote machines (stub, not yet implemented)

Multiple targets can be available simultaneously. The `target` param on each tool call selects which one.

## External access control

Targets are allowed/denied externally — Claude cannot change its own restrictions.

Options explored:
- Control file (`~/.tentacles/state.json`) + CLI (`tentaclesctl`)
- Web UI toggle in vibecraft dashboard
- Both read by the MCP server on each tool call

## Control plane as MCP server (future)

The vibecraft control plane itself could be exposed as an MCP server. A "supervisor" Claude instance could then manage other Claude instances programmatically — spawn sessions, send prompts, check status, read outputs — all via MCP tools. This enables hierarchical multi-agent orchestration.

## Implementation Status

**Resolved decisions:**
- MCP server is **embedded in the vibecraft server process** (not a separate binary). Each session gets its own HTTP server on a random port via `tentacles::start()`.
- Volume mounts are configurable per-target in the UI (host:container path pairs for image/dockerfile modes).
- Multiple targets per session (including multiple containers) are supported — Claude picks via `target` parameter.

**What's working:**
- Host target: direct execution on host machine
- Container target (existing): `docker exec` on a named container
- Container target (from image): `docker run -d` with volumes, then `docker exec`
- Container target (from Dockerfile): `docker build` + `docker run` with volumes
- Runtime target add/remove via REST API + direct registry access
- UI: Tentacles section in new session modal with enable toggle, host checkbox, container/SSH add buttons
- Container UI: 3 modes (existing, from image, from Dockerfile) with volume mapping

**Not yet implemented:**
- SSH target (stub returns "not yet implemented")
- External access control (lock targets from outside)
- Runtime target management UI in zone info modal (API exists, UI not wired)
- Control plane as MCP server (supervisor Claude)

## Open questions

- Voice control integration with Deepgram — keep or drop?
- How much of the 3D visualization is worth keeping vs. replacing with a simpler dashboard?
