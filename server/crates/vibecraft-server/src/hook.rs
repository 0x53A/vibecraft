//! Raw Claude Code hook event handling.
//!
//! Claude Code hooks pipe JSON to stdin with the native hook format.
//! This module accepts that raw JSON, transforms it into our internal
//! `ClaudeEvent` type, and handles transcript reading for response extraction.

use serde::Deserialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::ClaudeEvent;

// ── Raw hook event (Claude Code native format) ─────────────────────────────

/// Accepts any Claude Code hook event. Fields vary by hook type;
/// serde defaults handle missing fields gracefully.
#[derive(Debug, Deserialize)]
pub struct RawHookEvent {
    pub hook_event_name: String,
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,

    // Tool hooks (PreToolUse, PostToolUse)
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_response: Option<serde_json::Value>,

    // Stop hooks
    #[serde(default)]
    pub stop_hook_active: Option<bool>,
    #[serde(default)]
    pub transcript_path: Option<String>,

    // SessionStart
    #[serde(default)]
    pub source: Option<String>,

    // SessionEnd
    #[serde(default)]
    pub reason: Option<String>,

    // UserPromptSubmit
    #[serde(default)]
    pub prompt: Option<String>,

    // Notification
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,

    // PreCompact
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub custom_instructions: Option<String>,

    // PostToolUseFailure
    #[serde(default)]
    pub error: Option<String>,

    // SubagentStart / TaskCompleted
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,

    // PermissionRequest
    #[serde(default)]
    pub input: Option<serde_json::Value>,

    // TeammateIdle
    #[serde(default)]
    pub teammate_id: Option<String>,
}

// ── Conversion ──────────────────────────────────────────────────────────────

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn make_id(session_id: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{session_id}-{nanos}")
}

impl RawHookEvent {
    /// Convert a raw Claude Code hook event into our internal `ClaudeEvent`.
    /// For Stop events, reads the transcript file to extract the assistant response.
    pub async fn into_claude_event(self) -> Option<ClaudeEvent> {
        let timestamp = now_millis();
        let id = make_id(&self.session_id);

        match self.hook_event_name.as_str() {
            "PreToolUse" => {
                let assistant_text = self.read_assistant_text().await;
                Some(ClaudeEvent::PreToolUse {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    tool: self.tool_name.unwrap_or_else(|| "unknown".into()),
                    tool_input: self.tool_input.unwrap_or(serde_json::Value::Object(Default::default())),
                    tool_use_id: self.tool_use_id.unwrap_or_default(),
                    assistant_text,
                })
            }

            "PostToolUse" => {
                let tool_response = self.tool_response.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                let success = tool_response
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                Some(ClaudeEvent::PostToolUse {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    tool: self.tool_name.unwrap_or_else(|| "unknown".into()),
                    tool_input: self.tool_input.unwrap_or(serde_json::Value::Object(Default::default())),
                    tool_response,
                    tool_use_id: self.tool_use_id.unwrap_or_default(),
                    success,
                    duration: None,
                })
            }

            "Stop" => {
                let response = self.read_last_assistant_response_with_retry().await;
                Some(ClaudeEvent::Stop {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    stop_hook_active: self.stop_hook_active.unwrap_or(false),
                    response,
                })
            }

            "SubagentStop" => {
                Some(ClaudeEvent::SubagentStop {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    stop_hook_active: self.stop_hook_active.unwrap_or(false),
                })
            }

            "SessionStart" => {
                Some(ClaudeEvent::SessionStart {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    source: self.source.unwrap_or_else(|| "startup".into()),
                })
            }

            "SessionEnd" => {
                Some(ClaudeEvent::SessionEnd {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    reason: self.reason.unwrap_or_else(|| "other".into()),
                })
            }

            "UserPromptSubmit" => {
                Some(ClaudeEvent::UserPromptSubmit {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    prompt: self.prompt.unwrap_or_default(),
                })
            }

            "Notification" => {
                Some(ClaudeEvent::Notification {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    message: self.message.unwrap_or_default(),
                    notification_type: self.notification_type.unwrap_or_else(|| "unknown".into()),
                })
            }

            "PreCompact" => {
                Some(ClaudeEvent::PreCompact {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    trigger: self.trigger.unwrap_or_else(|| "manual".into()),
                    custom_instructions: self.custom_instructions,
                })
            }

            "PostToolUseFailure" => {
                Some(ClaudeEvent::PostToolUseFailure {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    tool: self.tool_name.unwrap_or_else(|| "unknown".into()),
                    tool_use_id: self.tool_use_id.unwrap_or_default(),
                    error: self.error.unwrap_or_default(),
                })
            }

            "SubagentStart" => {
                Some(ClaudeEvent::SubagentStart {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    tool_use_id: self.tool_use_id,
                    description: self.description,
                })
            }

            "PermissionRequest" => {
                Some(ClaudeEvent::PermissionRequest {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    tool: self.tool_name.unwrap_or_else(|| "unknown".into()),
                    input: self.input,
                })
            }

            "TaskCompleted" => {
                Some(ClaudeEvent::TaskCompleted {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    task: self.task,
                    success: self.success,
                })
            }

            "TeammateIdle" => {
                Some(ClaudeEvent::TeammateIdle {
                    id,
                    timestamp,
                    session_id: self.session_id,
                    cwd: self.cwd,
                    teammate_id: self.teammate_id,
                })
            }

            other => {
                tracing::debug!("Unknown hook event: {other}");
                None
            }
        }
    }

    /// Read the last assistant response from the transcript file, with retries.
    /// Claude Code may not have flushed the assistant message to disk yet when
    /// the Stop hook fires, so we retry a few times with short delays.
    async fn read_last_assistant_response_with_retry(&self) -> Option<String> {
        let path = self.transcript_path.as_deref()?;
        if path.is_empty() {
            return None;
        }

        // Try immediately first
        if let Some(response) = read_transcript_assistant_response(path, 200).await {
            return Some(response);
        }

        // Retry with increasing delays (50ms, 100ms, 200ms)
        for delay_ms in [50, 100, 200] {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            if let Some(response) = read_transcript_assistant_response(path, 200).await {
                tracing::debug!("Transcript response found after {delay_ms}ms retry");
                return Some(response);
            }
        }

        tracing::debug!("No assistant response found in transcript after retries: {path}");
        None
    }

    /// Read the assistant text preceding the current tool call.
    /// Used for PreToolUse events to capture context labels.
    async fn read_assistant_text(&self) -> Option<String> {
        let path = self.transcript_path.as_deref()?;
        if path.is_empty() {
            return None;
        }
        read_transcript_assistant_text(path, 30).await
    }
}

// ── Transcript reading ──────────────────────────────────────────────────────

/// Read the last assistant response from a Claude Code transcript JSONL file.
/// Scans the last `tail_lines` lines for the most recent assistant message.
/// Only returns the response if it appears AFTER the last user message,
/// to avoid returning a stale response from a previous turn.
async fn read_transcript_assistant_response(path: &str, tail_lines: usize) -> Option<String> {
    let path = Path::new(path);
    if !path.exists() {
        return None;
    }

    let content = tokio::fs::read_to_string(path).await.ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(tail_lines);
    let tail = &lines[start..];

    // Find positions of the last user message and last assistant message
    let mut last_user_idx: Option<usize> = None;
    let mut last_assistant_idx: Option<usize> = None;

    for (i, line) in tail.iter().enumerate() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            match entry.get("type").and_then(|t| t.as_str()) {
                Some("user") => last_user_idx = Some(i),
                Some("assistant") => {
                    if !extract_text_content(&entry).is_empty() {
                        last_assistant_idx = Some(i);
                    }
                }
                _ => {}
            }
        }
    }

    // Only return the assistant response if it came AFTER the last user message.
    // If the assistant response is before the last user message, the current
    // turn's response hasn't been flushed to disk yet — return None so the
    // retry logic can try again.
    let assistant_idx = last_assistant_idx?;
    if let Some(user_idx) = last_user_idx {
        if assistant_idx < user_idx {
            // Stale response from a previous turn
            return None;
        }
    }

    // Extract text from the assistant entry
    let entry = serde_json::from_str::<serde_json::Value>(tail[assistant_idx]).ok()?;
    let texts = extract_text_content(&entry);
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

/// Read the assistant text that appeared just before the current tool call.
/// Finds text after the last user message in the tail of the transcript.
async fn read_transcript_assistant_text(path: &str, tail_lines: usize) -> Option<String> {
    let path = Path::new(path);
    if !path.exists() {
        return None;
    }

    let content = tokio::fs::read_to_string(path).await.ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(tail_lines);
    let tail = &lines[start..];

    // Find the index of the last user message
    let mut last_user_idx: Option<usize> = None;
    for (i, line) in tail.iter().enumerate() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            if entry.get("type").and_then(|t| t.as_str()) == Some("user") {
                last_user_idx = Some(i);
            }
        }
    }

    let after_user = last_user_idx.map(|i| i + 1).unwrap_or(0);
    let mut texts = Vec::new();

    for line in &tail[after_user..] {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            if entry.get("type").and_then(|t| t.as_str()) == Some("assistant") {
                texts.extend(extract_text_content(&entry));
            }
        }
    }

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

/// Extract text content from a Claude Code transcript assistant entry.
fn extract_text_content(entry: &serde_json::Value) -> Vec<String> {
    entry
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        item.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
