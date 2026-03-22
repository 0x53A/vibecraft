#!/usr/bin/env bash
# Vibecraft Hook - Forwards raw Claude Code events to the Vibecraft server
#
# This script is called by Claude Code hooks and simply POSTs the raw
# hook event JSON to the Vibecraft server for processing. All event
# transformation, transcript reading, and persistence happens server-side.
#
# Installed to: ~/.vibecraft/hooks/vibecraft-hook.sh
# Run `npx vibecraft setup` to install/update this hook.

# Read raw hook event from stdin
input=$(cat)

VIBECRAFT_BASE="http://localhost:${VIBECRAFT_PORT:-4003}"

# Find curl - check common locations if not in PATH
CURL=$(command -v curl 2>/dev/null)
if [ -z "$CURL" ]; then
  for dir in /usr/bin /usr/local/bin /opt/homebrew/bin "$HOME/.local/bin"; do
    [ -x "$dir/curl" ] && CURL="$dir/curl" && break
  done
fi
[ -z "$CURL" ] && exit 0

# Build headers
HEADERS=(-H "Content-Type: application/json")

# Send tmux session name so server can match to managed sessions.
# Works for both newly-created and adopted sessions.
if [ -n "$TMUX" ]; then
  TMUX_NAME=$(tmux display-message -p '#{session_name}' 2>/dev/null)
  [ -n "$TMUX_NAME" ] && HEADERS+=(-H "X-Tmux-Session: $TMUX_NAME")
fi

# POST raw event to server (fire and forget, don't block Claude)
"$CURL" -s -X POST "${VIBECRAFT_BASE}/hook" \
  "${HEADERS[@]}" \
  -d "$input" \
  --connect-timeout 1 \
  --max-time 2 \
  >/dev/null 2>&1 &

exit 0
