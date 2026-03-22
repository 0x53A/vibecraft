//! Tmux interaction: sending commands, parsing output, detecting prompts.

use std::process::Output;
use std::time::Duration;

use anyhow::{bail, Context};
use regex::Regex;
use tokio::process::Command;

use crate::types::PermissionOption;

/// Validate tmux session name (alphanumeric, underscore, hyphen only).
pub fn validate_tmux_session(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        bail!("tmux session name cannot be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!(
            "invalid tmux session name '{}': only alphanumeric, underscore, and hyphen allowed",
            name
        );
    }
    Ok(())
}

/// Execute a command with an extended PATH.
pub async fn exec_with_path(cmd: &str, args: &[&str], path: &str) -> anyhow::Result<Output> {
    let output = Command::new(cmd)
        .args(args)
        .env("PATH", path)
        .output()
        .await
        .with_context(|| format!("failed to execute `{cmd}`"))?;
    Ok(output)
}

/// Safely send text to a tmux session using load-buffer + paste-buffer.
///
/// This avoids shell interpretation issues by writing text to a temp file,
/// loading it into a tmux buffer, then pasting into the target pane.
pub async fn send_to_tmux_safe(
    tmux_session: &str,
    text: &str,
    exec_path: &str,
) -> anyhow::Result<()> {
    validate_tmux_session(tmux_session)?;

    // Write text to a temp file
    let tmp = tempfile::NamedTempFile::new().context("failed to create temp file")?;
    let tmp_path = tmp.path().to_owned();
    tokio::fs::write(&tmp_path, text)
        .await
        .context("failed to write temp file")?;

    let tmp_str = tmp_path
        .to_str()
        .context("temp path is not valid UTF-8")?;

    // Load into tmux buffer
    let load = exec_with_path("tmux", &["load-buffer", tmp_str], exec_path).await?;
    if !load.status.success() {
        let stderr = String::from_utf8_lossy(&load.stderr);
        bail!("tmux load-buffer failed: {stderr}");
    }

    // Paste into session
    let paste = exec_with_path(
        "tmux",
        &["paste-buffer", "-t", tmux_session],
        exec_path,
    )
    .await?;
    if !paste.status.success() {
        let stderr = String::from_utf8_lossy(&paste.stderr);
        bail!("tmux paste-buffer failed: {stderr}");
    }

    // Small delay before sending Enter
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send Enter
    let enter = exec_with_path(
        "tmux",
        &["send-keys", "-t", tmux_session, "Enter"],
        exec_path,
    )
    .await?;
    if !enter.status.success() {
        let stderr = String::from_utf8_lossy(&enter.stderr);
        bail!("tmux send-keys failed: {stderr}");
    }

    // Cleanup happens on drop of NamedTempFile (tmp)
    drop(tmp);

    Ok(())
}

/// Parse token count from Claude Code output.
///
/// Matches patterns like "↓ 879 tokens", "↓ 12.5k tokens", "↓ 1,234 tokens".
/// Returns the maximum found across all matches.
pub fn parse_tokens_from_output(output: &str) -> Option<u64> {
    let plain_re = Regex::new(r"↓\s*([0-9,]+)\s*tokens?").unwrap();
    let kilo_re = Regex::new(r"↓\s*([0-9.]+)k\s*tokens?").unwrap();

    let mut max_tokens: Option<u64> = None;

    for cap in plain_re.captures_iter(output) {
        let num_str = cap[1].replace(',', "");
        if let Ok(n) = num_str.parse::<u64>() {
            max_tokens = Some(max_tokens.map_or(n, |m: u64| m.max(n)));
        }
    }

    for cap in kilo_re.captures_iter(output) {
        if let Ok(f) = cap[1].parse::<f64>() {
            let n = (f * 1000.0) as u64;
            max_tokens = Some(max_tokens.map_or(n, |m: u64| m.max(n)));
        }
    }

    max_tokens
}

/// Detect a permission prompt in tmux output.
///
/// Returns `(tool, context, options)` if a permission prompt is found.
pub fn detect_permission_prompt(output: &str) -> Option<(String, String, Vec<PermissionOption>)> {
    let lines: Vec<&str> = output.lines().collect();
    let len = lines.len();
    if len == 0 {
        return None;
    }

    // Look in the last 30 lines
    let start = len.saturating_sub(30);
    let tail = &lines[start..];

    // Check for prompt question - Claude Code uses variable prompts like:
    // "Do you want to create file.txt?"
    // "Do you want to proceed?"
    // "Would you like to proceed?"
    let has_prompt = tail.iter().any(|l| {
        l.contains("Do you want to") || l.contains("Would you like to")
    });
    if !has_prompt {
        return None;
    }

    // Verify footer is near the bottom (last 5 non-empty lines).
    // When the prompt is active, "Esc to cancel" is at the very end.
    // After the user answers, new output pushes it up — so checking
    // only the last few lines prevents stale matches from scrollback.
    let last_lines: Vec<&str> = tail.iter().rev().take(5).copied().collect();
    let has_footer = last_lines
        .iter()
        .any(|l| l.contains("Esc to cancel") || l.contains("ctrl-g"));
    if !has_footer {
        return None;
    }

    // Parse numbered options: lines like "  1. Allow once" or " ❯ 2. Allow always"
    let option_re = Regex::new(r"^\s*[❯>]?\s*(\d+)\.\s+(.+)$").unwrap();
    let mut options = Vec::new();
    for line in tail {
        if let Some(cap) = option_re.captures(line) {
            options.push(PermissionOption {
                number: cap[1].to_string(),
                label: cap[2].trim().to_string(),
            });
        }
    }

    if options.len() < 2 {
        return None;
    }

    // Find tool name by searching backwards for spinner pattern: [●◐·] ToolName (
    let tool_re = Regex::new(r"[●◐·]\s*(\w+)\s*\(").unwrap();
    let mut tool = String::new();
    for line in lines.iter().rev() {
        if let Some(cap) = tool_re.captures(line) {
            tool = cap[1].to_string();
            break;
        }
    }

    // Gather context: lines between the tool and the prompt
    let context = tail
        .iter()
        .take_while(|l| {
            !l.contains("Do you want to") && !l.contains("Would you like to")
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n");

    Some((tool, context, options))
}

/// Detect the "trust this folder" prompt that appears when Claude Code
/// enters a new workspace for the first time.
/// Only matches when the prompt is at the bottom of the terminal (last 10 lines).
pub fn detect_trust_prompt(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.len().saturating_sub(10);
    lines[start..]
        .iter()
        .any(|l| l.contains("Yes, I trust this folder"))
}

/// Detect bypass permissions warning in tmux output.
/// Only matches when the warning is at the bottom of the terminal (last 10 lines).
pub fn detect_bypass_warning(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.len().saturating_sub(10);
    lines[start..]
        .iter()
        .any(|l| l.contains("bypass") && l.contains("permission"))
}

/// Validate a directory path: must exist, be a directory, and contain no shell metacharacters.
pub fn validate_directory_path(input_path: &str) -> anyhow::Result<String> {
    if input_path.is_empty() {
        bail!("directory path cannot be empty");
    }

    // Check for shell metacharacters
    let forbidden = [';', '&', '|', '`', '$', '(', ')', '{', '}', '<', '>', '!', '\\', '*', '?', '[', ']', '\n', '\r'];
    for ch in forbidden {
        if input_path.contains(ch) {
            bail!(
                "directory path contains forbidden character '{}'",
                ch.escape_default()
            );
        }
    }

    // Expand ~ to home
    let expanded = if let Some(rest) = input_path.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME not set")?;
        format!("{home}/{rest}")
    } else if input_path == "~" {
        std::env::var("HOME").context("HOME not set")?
    } else {
        input_path.to_string()
    };

    // Canonicalize and verify
    let canonical = std::fs::canonicalize(&expanded)
        .with_context(|| format!("path does not exist: {expanded}"))?;

    if !canonical.is_dir() {
        bail!("path is not a directory: {}", canonical.display());
    }

    Ok(canonical
        .to_str()
        .context("path is not valid UTF-8")?
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_tmux_session() {
        assert!(validate_tmux_session("claude").is_ok());
        assert!(validate_tmux_session("my-session_01").is_ok());
        assert!(validate_tmux_session("").is_err());
        assert!(validate_tmux_session("foo bar").is_err());
        assert!(validate_tmux_session("foo;bar").is_err());
    }

    #[test]
    fn test_parse_tokens_plain() {
        assert_eq!(parse_tokens_from_output("↓ 879 tokens"), Some(879));
        assert_eq!(parse_tokens_from_output("↓ 1,234 tokens"), Some(1234));
    }

    #[test]
    fn test_parse_tokens_kilo() {
        assert_eq!(parse_tokens_from_output("↓ 12.5k tokens"), Some(12500));
    }

    #[test]
    fn test_parse_tokens_max() {
        assert_eq!(
            parse_tokens_from_output("↓ 500 tokens ... ↓ 2k tokens"),
            Some(2000)
        );
    }

    #[test]
    fn test_parse_tokens_none() {
        assert_eq!(parse_tokens_from_output("no tokens here"), None);
    }

    #[test]
    fn test_detect_bypass_warning() {
        assert!(detect_bypass_warning("warning: bypass permission check"));
        assert!(!detect_bypass_warning("everything is fine"));
    }

    #[test]
    fn test_detect_permission_prompt_basic() {
        let output = r#"
● Bash (ls /tmp)
Some context line

Do you want to proceed?
  1. Allow once
  2. Allow always
  3. Deny

Press Esc to cancel
"#;
        let result = detect_permission_prompt(output);
        assert!(result.is_some());
        let (tool, _context, options) = result.unwrap();
        assert_eq!(tool, "Bash");
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].number, "1");
        assert_eq!(options[0].label, "Allow once");
    }

    #[test]
    fn test_detect_permission_prompt_missing_footer() {
        let output = "Do you want to proceed?\n  1. Allow once\n  2. Deny\n";
        assert!(detect_permission_prompt(output).is_none());
    }

    #[test]
    fn test_validate_directory_path_rejects_metacharacters() {
        assert!(validate_directory_path("/tmp; rm -rf /").is_err());
        assert!(validate_directory_path("/tmp$(whoami)").is_err());
    }

    #[test]
    fn test_validate_directory_path_accepts_tmp() {
        // /tmp should exist on any system
        let result = validate_directory_path("/tmp");
        assert!(result.is_ok());
    }
}
