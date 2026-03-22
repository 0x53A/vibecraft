//! Git status polling for tracked session directories.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::process::Command;

use crate::types::{FileChangeCounts, GitStatus};

/// Manages git status polling for tracked session directories.
pub struct GitStatusManager {
    directories: HashMap<String, String>,
    cache: HashMap<String, GitStatus>,
    poll_interval: Duration,
    exec_timeout: Duration,
}

impl GitStatusManager {
    pub fn new() -> Self {
        Self {
            directories: HashMap::new(),
            cache: HashMap::new(),
            poll_interval: Duration::from_secs(5),
            exec_timeout: Duration::from_secs(5),
        }
    }

    /// Register a directory to track for a session.
    pub fn track(&mut self, session_id: String, directory: String) {
        self.directories.insert(session_id, directory);
    }

    /// Stop tracking a session.
    pub fn untrack(&mut self, session_id: &str) {
        self.directories.remove(session_id);
        self.cache.remove(session_id);
    }

    /// Get cached git status for a session.
    pub fn get_status(&self, session_id: &str) -> Option<&GitStatus> {
        self.cache.get(session_id)
    }

    /// The configured poll interval.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Poll git status for all tracked directories, updating the cache.
    pub async fn poll_all(&mut self) {
        let dirs: Vec<(String, String)> = self
            .directories
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (session_id, directory) in dirs {
            let status = Self::fetch_status_inner(&directory, self.exec_timeout).await;
            self.cache.insert(session_id, status);
        }
    }

    /// Fetch git status for a single directory.
    pub async fn fetch_status(&self, directory: &str) -> GitStatus {
        Self::fetch_status_inner(directory, self.exec_timeout).await
    }

    async fn fetch_status_inner(directory: &str, timeout: Duration) -> GitStatus {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let dir = directory.to_string();

        // Check if this is a git repo first — avoid noisy warnings on non-repo dirs
        if run_git(&dir, &["rev-parse", "--git-dir"], timeout).await.is_none() {
            return GitStatus {
                last_checked: now,
                ..Default::default()
            };
        }

        // Run remaining git commands in parallel
        let (branch, porcelain, cached_stat, unstaged_stat, log_out, upstream) =
            tokio::join!(
                run_git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"], timeout),
                run_git(&dir, &["status", "--porcelain"], timeout),
                run_git(&dir, &["diff", "--cached", "--shortstat"], timeout),
                run_git(&dir, &["diff", "--shortstat"], timeout),
                run_git(&dir, &["log", "-1", "--format=%ct|||%s"], timeout),
                run_git(
                    &dir,
                    &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
                    timeout,
                ),
            );

        let branch = branch.unwrap_or_default().trim().to_string();

        // Parse porcelain output
        let (staged, unstaged, untracked) = parse_porcelain(&porcelain.unwrap_or_default());

        // Parse shortstat outputs
        let (staged_lines_add, staged_lines_del) =
            parse_shortstat(&cached_stat.unwrap_or_default());
        let (unstaged_lines_add, unstaged_lines_del) =
            parse_shortstat(&unstaged_stat.unwrap_or_default());
        let lines_added = staged_lines_add + unstaged_lines_add;
        let lines_removed = staged_lines_del + unstaged_lines_del;

        // Parse last commit
        let (last_commit_time, last_commit_message) =
            parse_log_output(&log_out.unwrap_or_default());

        // Parse ahead/behind
        let (behind, ahead) = parse_upstream(&upstream.unwrap_or_default());

        let total_files =
            staged.added + staged.modified + staged.deleted +
            unstaged.added + unstaged.modified + unstaged.deleted +
            untracked;

        GitStatus {
            branch,
            ahead,
            behind,
            staged,
            unstaged,
            untracked,
            total_files,
            lines_added,
            lines_removed,
            last_commit_time,
            last_commit_message,
            is_repo: true,
            last_checked: now,
        }
    }
}

/// Run a git command in the given directory with a timeout.
/// Returns `None` if the command fails or times out.
async fn run_git(dir: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let result = tokio::time::timeout(
        timeout,
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Ok(_output)) => {
            // Non-zero exit is expected for non-git dirs, upstream checks, etc.
            None
        }
        Ok(Err(e)) => {
            tracing::debug!("git {:?} in {dir} exec error: {e}", args);
            None
        }
        Err(_) => {
            tracing::debug!("git {:?} in {dir} timed out", args);
            None
        }
    }
}

/// Parse `git status --porcelain` output into staged/unstaged/untracked counts.
fn parse_porcelain(output: &str) -> (FileChangeCounts, FileChangeCounts, u32) {
    let mut staged = FileChangeCounts { added: 0, modified: 0, deleted: 0 };
    let mut unstaged = FileChangeCounts { added: 0, modified: 0, deleted: 0 };
    let mut untracked: u32 = 0;

    for line in output.lines() {
        if line.len() < 2 {
            continue;
        }
        let bytes = line.as_bytes();
        let x = bytes[0] as char; // staging area
        let y = bytes[1] as char; // working tree

        if x == '?' && y == '?' {
            untracked += 1;
            continue;
        }

        // Staged changes (index column)
        match x {
            'A' | 'C' => staged.added += 1,
            'M' | 'R' | 'T' => staged.modified += 1,
            'D' => staged.deleted += 1,
            _ => {}
        }

        // Unstaged changes (working tree column)
        match y {
            'A' => unstaged.added += 1,
            'M' | 'R' | 'T' => unstaged.modified += 1,
            'D' => unstaged.deleted += 1,
            _ => {}
        }
    }

    (staged, unstaged, untracked)
}

/// Parse `git diff --shortstat` output.
/// Example: " 3 files changed, 10 insertions(+), 5 deletions(-)"
fn parse_shortstat(output: &str) -> (u32, u32) {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return (0, 0);
    }

    let mut insertions: u32 = 0;
    let mut deletions: u32 = 0;

    // Split by comma and look for insertions/deletions
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.contains("insertion") {
            if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok()) {
                insertions = n;
            }
        } else if part.contains("deletion") {
            if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok()) {
                deletions = n;
            }
        }
    }

    (insertions, deletions)
}

/// Parse `git log -1 --format=%ct|||%s` output.
fn parse_log_output(output: &str) -> (Option<i64>, Option<String>) {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return (None, None);
    }

    let parts: Vec<&str> = trimmed.splitn(2, "|||").collect();
    let time = parts.first().and_then(|s| s.parse::<i64>().ok());
    let message = parts.get(1).map(|s| s.to_string());

    (time, message)
}

/// Parse `git rev-list --left-right --count @{upstream}...HEAD`.
/// Output: "3\t5" means 3 behind, 5 ahead.
fn parse_upstream(output: &str) -> (u32, u32) {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return (0, 0);
    }

    let parts: Vec<&str> = trimmed.split('\t').collect();
    let behind = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let ahead = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

    (behind, ahead)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain() {
        let output = "\
M  src/main.rs
A  src/new.rs
 M src/lib.rs
 D src/old.rs
?? untracked.txt
?? another.txt
";
        let (staged, unstaged, untracked) = parse_porcelain(output);
        assert_eq!(staged.modified, 1); // M_
        assert_eq!(staged.added, 1); // A_
        assert_eq!(unstaged.modified, 1); // _M
        assert_eq!(unstaged.deleted, 1); // _D
        assert_eq!(untracked, 2);
    }

    #[test]
    fn test_parse_shortstat() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 10 insertions(+), 5 deletions(-)"),
            (10, 5)
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 3 insertions(+)"),
            (3, 0)
        );
        assert_eq!(parse_shortstat(""), (0, 0));
    }

    #[test]
    fn test_parse_log_output() {
        let (time, msg) = parse_log_output("1700000000|||fix: resolve issue");
        assert_eq!(time, Some(1700000000));
        assert_eq!(msg.as_deref(), Some("fix: resolve issue"));
    }

    #[test]
    fn test_parse_log_output_with_separator_in_message() {
        let (time, msg) = parse_log_output("1700000000|||foo|||bar");
        assert_eq!(time, Some(1700000000));
        assert_eq!(msg.as_deref(), Some("foo|||bar"));
    }

    #[test]
    fn test_parse_upstream() {
        assert_eq!(parse_upstream("3\t5"), (3, 5));
        assert_eq!(parse_upstream("0\t0"), (0, 0));
        assert_eq!(parse_upstream(""), (0, 0));
    }
}
