use std::env;
use std::path::PathBuf;

/// Replace leading `~` with the user's home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub port: u16,
    pub events_file: PathBuf,
    pub sessions_file: PathBuf,
    pub tiles_file: PathBuf,
    pub templates_file: PathBuf,
    pub pending_prompt_file: PathBuf,
    pub max_events: usize,
    pub debug: bool,
    pub tmux_session: String,
    pub working_timeout_ms: u64,
    pub working_timeout_check_interval_ms: u64,
    pub max_body_size: usize,
    pub exec_path: String,
}

impl Config {
    pub fn from_env() -> Self {
        let data_dir = expand_home("~/.vibecraft/data");

        let port = env::var("VIBECRAFT_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4003);

        let events_file = env::var("VIBECRAFT_EVENTS_FILE")
            .map(|v| expand_home(&v))
            .unwrap_or_else(|_| data_dir.join("events.jsonl"));

        let sessions_file = env::var("VIBECRAFT_SESSIONS_FILE")
            .map(|v| expand_home(&v))
            .unwrap_or_else(|_| data_dir.join("sessions.json"));

        let tiles_file = data_dir.join("tiles.json");
        let templates_file = data_dir.join("templates.json");
        let pending_prompt_file = data_dir.join("pending-prompt.txt");

        let max_events = env::var("VIBECRAFT_MAX_EVENTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);

        let debug = env::var("VIBECRAFT_DEBUG")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let tmux_session = env::var("VIBECRAFT_TMUX_SESSION")
            .unwrap_or_else(|_| "claude".to_string());

        let exec_path = env::var("PATH").unwrap_or_default();

        Self {
            port,
            events_file,
            sessions_file,
            tiles_file,
            templates_file,
            pending_prompt_file,
            max_events,
            debug,
            tmux_session,
            working_timeout_ms: 120_000,
            working_timeout_check_interval_ms: 10_000,
            max_body_size: 1_048_576, // 1 MB
            exec_path,
        }
    }
}
