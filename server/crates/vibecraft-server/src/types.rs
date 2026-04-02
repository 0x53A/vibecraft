//! Shared types mirroring shared/types.ts
//! This is the Rust-side contract for events, sessions, messages, etc.

use serde::{Deserialize, Serialize};

// ── Event types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEventType {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    Stop,
    SubagentStart,
    SubagentStop,
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    Notification,
    PreCompact,
    PermissionRequest,
    TaskCompleted,
    TeammateIdle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeEvent {
    PreToolUse {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        tool: String,
        #[serde(rename = "toolInput")]
        tool_input: serde_json::Value,
        #[serde(rename = "toolUseId")]
        tool_use_id: String,
        #[serde(rename = "assistantText")]
        #[serde(skip_serializing_if = "Option::is_none")]
        assistant_text: Option<String>,
    },
    PostToolUse {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        tool: String,
        #[serde(rename = "toolInput")]
        tool_input: serde_json::Value,
        #[serde(rename = "toolResponse")]
        tool_response: serde_json::Value,
        #[serde(rename = "toolUseId")]
        tool_use_id: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<u64>,
    },
    Stop {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        #[serde(rename = "stopHookActive")]
        stop_hook_active: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<String>,
    },
    SubagentStop {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        #[serde(rename = "stopHookActive")]
        stop_hook_active: bool,
    },
    SessionStart {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        source: String,
    },
    SessionEnd {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        reason: String,
    },
    UserPromptSubmit {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        prompt: String,
    },
    Notification {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        message: String,
        #[serde(rename = "notificationType")]
        notification_type: String,
    },
    PreCompact {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        trigger: String,
        #[serde(rename = "customInstructions")]
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    PostToolUseFailure {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        tool: String,
        #[serde(rename = "toolUseId")]
        tool_use_id: String,
        error: String,
    },
    SubagentStart {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        #[serde(rename = "toolUseId")]
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    PermissionRequest {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        tool: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    TaskCompleted {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        task: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        success: Option<bool>,
    },
    TeammateIdle {
        id: String,
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        cwd: String,
        #[serde(rename = "teammateId")]
        #[serde(skip_serializing_if = "Option::is_none")]
        teammate_id: Option<String>,
    },
}

impl ClaudeEvent {
    pub fn id(&self) -> &str {
        match self {
            Self::PreToolUse { id, .. }
            | Self::PostToolUse { id, .. }
            | Self::PostToolUseFailure { id, .. }
            | Self::Stop { id, .. }
            | Self::SubagentStart { id, .. }
            | Self::SubagentStop { id, .. }
            | Self::SessionStart { id, .. }
            | Self::SessionEnd { id, .. }
            | Self::UserPromptSubmit { id, .. }
            | Self::Notification { id, .. }
            | Self::PreCompact { id, .. }
            | Self::PermissionRequest { id, .. }
            | Self::TaskCompleted { id, .. }
            | Self::TeammateIdle { id, .. } => id,
        }
    }

    pub fn session_id(&self) -> &str {
        match self {
            Self::PreToolUse { session_id, .. }
            | Self::PostToolUse { session_id, .. }
            | Self::PostToolUseFailure { session_id, .. }
            | Self::Stop { session_id, .. }
            | Self::SubagentStart { session_id, .. }
            | Self::SubagentStop { session_id, .. }
            | Self::SessionStart { session_id, .. }
            | Self::SessionEnd { session_id, .. }
            | Self::UserPromptSubmit { session_id, .. }
            | Self::Notification { session_id, .. }
            | Self::PreCompact { session_id, .. }
            | Self::PermissionRequest { session_id, .. }
            | Self::TaskCompleted { session_id, .. }
            | Self::TeammateIdle { session_id, .. } => session_id,
        }
    }

    pub fn cwd(&self) -> &str {
        match self {
            Self::PreToolUse { cwd, .. }
            | Self::PostToolUse { cwd, .. }
            | Self::PostToolUseFailure { cwd, .. }
            | Self::Stop { cwd, .. }
            | Self::SubagentStart { cwd, .. }
            | Self::SubagentStop { cwd, .. }
            | Self::SessionStart { cwd, .. }
            | Self::SessionEnd { cwd, .. }
            | Self::UserPromptSubmit { cwd, .. }
            | Self::Notification { cwd, .. }
            | Self::PreCompact { cwd, .. }
            | Self::PermissionRequest { cwd, .. }
            | Self::TaskCompleted { cwd, .. }
            | Self::TeammateIdle { cwd, .. } => cwd,
        }
    }

    pub fn event_type(&self) -> &str {
        match self {
            Self::PreToolUse { .. } => "pre_tool_use",
            Self::PostToolUse { .. } => "post_tool_use",
            Self::PostToolUseFailure { .. } => "post_tool_use_failure",
            Self::Stop { .. } => "stop",
            Self::SubagentStart { .. } => "subagent_start",
            Self::SubagentStop { .. } => "subagent_stop",
            Self::SessionStart { .. } => "session_start",
            Self::SessionEnd { .. } => "session_end",
            Self::UserPromptSubmit { .. } => "user_prompt_submit",
            Self::Notification { .. } => "notification",
            Self::PreCompact { .. } => "pre_compact",
            Self::PermissionRequest { .. } => "permission_request",
            Self::TaskCompleted { .. } => "task_completed",
            Self::TeammateIdle { .. } => "teammate_idle",
        }
    }

    pub fn timestamp(&self) -> u64 {
        match self {
            Self::PreToolUse { timestamp, .. }
            | Self::PostToolUse { timestamp, .. }
            | Self::PostToolUseFailure { timestamp, .. }
            | Self::Stop { timestamp, .. }
            | Self::SubagentStart { timestamp, .. }
            | Self::SubagentStop { timestamp, .. }
            | Self::SessionStart { timestamp, .. }
            | Self::SessionEnd { timestamp, .. }
            | Self::UserPromptSubmit { timestamp, .. }
            | Self::Notification { timestamp, .. }
            | Self::PreCompact { timestamp, .. }
            | Self::PermissionRequest { timestamp, .. }
            | Self::TaskCompleted { timestamp, .. }
            | Self::TeammateIdle { timestamp, .. } => *timestamp,
        }
    }
}

// ── Session types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Working,
    Waiting,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSession {
    pub id: String,
    pub name: String,
    pub tmux_session: String,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    pub created_at: u64,
    pub last_activity: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_status: Option<GitStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone_position: Option<HexPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_permission: Option<PendingPermission>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tentacles: Option<TentaclesSessionInfo>,
    /// The flags used to spawn this session (persisted for restart/reconnect).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_flags: Option<SessionFlags>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPermission {
    pub id: String,
    pub tool: String,
    pub context: String,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub current: u64,
    pub cumulative: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexPosition {
    pub q: i32,
    pub r: i32,
}

// ── Git status ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: FileChangeCounts,
    pub unstaged: FileChangeCounts,
    pub untracked: u32,
    pub total_files: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub last_commit_time: Option<i64>,
    pub last_commit_message: Option<String>,
    pub is_repo: bool,
    pub last_checked: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChangeCounts {
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
}

impl Default for GitStatus {
    fn default() -> Self {
        Self {
            branch: String::new(),
            ahead: 0,
            behind: 0,
            staged: FileChangeCounts { added: 0, modified: 0, deleted: 0 },
            unstaged: FileChangeCounts { added: 0, modified: 0, deleted: 0 },
            untracked: 0,
            total_files: 0,
            lines_added: 0,
            lines_removed: 0,
            last_commit_time: None,
            last_commit_message: None,
            is_repo: false,
            last_checked: 0,
        }
    }
}

// ── Text tiles ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextTile {
    pub id: String,
    pub text: String,
    pub position: HexPosition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub created_at: u64,
}

// ── WebSocket messages ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerMessage {
    Event(ClaudeEvent),
    History(Vec<ClaudeEvent>),
    Connected { #[serde(rename = "sessionId")] session_id: String },
    Error { message: String },
    Tokens { session: String, current: u64, cumulative: u64 },
    Sessions(Vec<ManagedSession>),
    SessionUpdate(ManagedSession),
    PermissionPrompt {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "permissionId")]
        permission_id: String,
        tool: String,
        context: String,
        options: Vec<PermissionOption>,
    },
    PermissionResolved {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    TextTiles(Vec<TextTile>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionOption {
    pub number: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe {
        #[serde(default)]
        payload: Option<serde_json::Value>,
    },
    GetHistory {
        #[serde(default)]
        payload: Option<GetHistoryPayload>,
    },
    Ping,
    VoiceStart,
    VoiceStop,
    PermissionResponse {
        payload: PermissionResponsePayload,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetHistoryPayload {
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionResponsePayload {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "permissionId")]
    pub permission_id: String,
    pub response: String,
}

// ── HTTP request/response types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub flags: Option<SessionFlags>,
    #[serde(default)]
    pub resume: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumableSession {
    pub session_id: String,
    pub cwd: String,
    pub started_at: u64,
    pub pid: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFlags {
    #[serde(rename = "continue")]
    pub continue_session: Option<bool>,
    pub skip_permissions: Option<bool>,
    pub chrome: Option<bool>,
    pub tools: Option<Vec<String>>,
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    pub tentacles: Option<TentaclesConfig>,
    /// "default" | "append" | "replace"
    pub system_prompt_mode: Option<String>,
    pub system_prompt_text: Option<String>,
    /// Whether auto-memory is enabled (default true)
    pub memory: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TentaclesConfig {
    pub enabled: bool,
    #[serde(default)]
    pub targets: Vec<TentaclesTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TentaclesTarget {
    pub name: String,
    pub target_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TentaclesSessionInfo {
    pub enabled: bool,
    pub port: u16,
    pub targets: Vec<TentaclesTarget>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSessionRequest {
    pub name: Option<String>,
    #[serde(rename = "zonePosition")]
    pub zone_position: Option<HexPosition>,
}

#[derive(Debug, Deserialize)]
pub struct SessionPromptRequest {
    pub prompt: String,
    #[serde(default)]
    pub send: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct PromptRequest {
    pub prompt: String,
    #[serde(default)]
    pub send: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LinkSessionRequest {
    #[serde(rename = "claudeSessionId")]
    pub claude_session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTextTileRequest {
    pub text: String,
    pub position: HexPosition,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTextTileRequest {
    pub text: Option<String>,
    pub position: Option<HexPosition>,
    pub color: Option<String>,
}

// ── System prompt templates ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplate {
    pub name: String,
    pub text: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct SaveTemplateRequest {
    pub name: String,
    pub text: String,
}

// ── Known projects ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownProject {
    pub path: String,
    pub name: String,
    pub last_used: u64,
    pub use_count: u32,
}
