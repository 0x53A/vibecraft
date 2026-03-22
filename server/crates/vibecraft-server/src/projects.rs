//! Project directory tracking and autocomplete.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use tracing::warn;

use crate::types::KnownProject;

/// Manages known project directories for autocomplete and recency tracking.
pub struct ProjectsManager {
    config_file: PathBuf,
    projects: Vec<KnownProject>,
}

impl ProjectsManager {
    /// Create a new ProjectsManager, loading from disk.
    pub fn new() -> Self {
        let config_file = dirs_config_path();
        let mut mgr = Self {
            config_file,
            projects: Vec::new(),
        };
        if let Err(e) = mgr.load() {
            warn!("failed to load projects: {e}");
        }
        mgr
    }

    /// Get all known projects, sorted by most recently used first.
    pub fn get_projects(&self) -> Vec<KnownProject> {
        let mut sorted = self.projects.clone();
        sorted.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        sorted
    }

    /// Add a project or update its last-used timestamp and use count.
    pub fn add_project(&mut self, path: &str, name: &str) {
        let now = now_millis();

        if let Some(existing) = self.projects.iter_mut().find(|p| p.path == path) {
            existing.name = name.to_string();
            existing.last_used = now;
            existing.use_count += 1;
        } else {
            self.projects.push(KnownProject {
                path: path.to_string(),
                name: name.to_string(),
                last_used: now,
                use_count: 1,
            });
        }

        if let Err(e) = self.save() {
            warn!("failed to save projects: {e}");
        }
    }

    /// Remove a project by path.
    pub fn remove_project(&mut self, path: &str) {
        self.projects.retain(|p| p.path != path);
        if let Err(e) = self.save() {
            warn!("failed to save projects: {e}");
        }
    }

    /// Autocomplete a partial path/name.
    ///
    /// If the input looks like a path (starts with `/`, `~`, `.`), does filesystem
    /// completion. Also matches against known project paths and names.
    /// Returns up to `limit` results.
    pub fn autocomplete(&self, partial: &str, limit: usize) -> Vec<String> {
        let is_path_like = partial.starts_with('/')
            || partial.starts_with('~')
            || partial.starts_with('.');
        let is_browsing = partial.ends_with('/');

        let mut fs_results = Vec::new();

        // Filesystem completion for path-like inputs
        if is_path_like {
            fs_results = filesystem_complete(partial);
        }

        // Known project matching (fuzzy: path or name contains partial)
        let known_results: Vec<String>;
        if !partial.is_empty() {
            let lower = partial.to_lowercase();
            let mut projects = self.get_projects();
            projects.retain(|p| {
                p.path.to_lowercase().contains(&lower)
                    || p.name.to_lowercase().contains(&lower)
            });
            known_results = projects.into_iter().map(|p| p.path).collect();
        } else {
            // Empty partial: return recent projects
            known_results = self
                .get_projects()
                .into_iter()
                .map(|p| p.path)
                .collect();
        }

        // Combine: if browsing, filesystem first; otherwise known projects first
        let mut combined = Vec::new();
        let mut seen = std::collections::HashSet::new();

        if is_browsing {
            for item in fs_results.iter().chain(known_results.iter()) {
                if seen.insert(item.clone()) {
                    combined.push(item.clone());
                }
            }
        } else {
            for item in known_results.iter().chain(fs_results.iter()) {
                if seen.insert(item.clone()) {
                    combined.push(item.clone());
                }
            }
        }

        combined.truncate(limit);
        combined
    }

    /// Load projects from disk.
    pub fn load(&mut self) -> anyhow::Result<()> {
        if !self.config_file.exists() {
            self.projects = Vec::new();
            return Ok(());
        }

        let data = std::fs::read_to_string(&self.config_file)
            .with_context(|| format!("reading {}", self.config_file.display()))?;

        // Try wrapped format { "projects": [...] } first, then plain array
        #[derive(serde::Deserialize)]
        struct Wrapped {
            projects: Vec<KnownProject>,
        }

        self.projects = if let Ok(w) = serde_json::from_str::<Wrapped>(&data) {
            w.projects
        } else {
            serde_json::from_str(&data)
                .with_context(|| format!("parsing {}", self.config_file.display()))?
        };

        Ok(())
    }

    /// Save projects to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.config_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let data = serde_json::to_string_pretty(&self.projects)
            .context("serializing projects")?;

        std::fs::write(&self.config_file, data)
            .with_context(|| format!("writing {}", self.config_file.display()))?;

        Ok(())
    }
}

/// Resolve the config file path: `~/.vibecraft/projects.json`.
fn dirs_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".vibecraft")
        .join("projects.json")
}

/// Get current time in milliseconds since epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Filesystem directory completion.
///
/// Given a partial path, lists matching directories. Uses sync I/O.
fn filesystem_complete(partial: &str) -> Vec<String> {
    // Expand ~
    let expanded = if let Some(rest) = partial.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/{rest}")
    } else if partial == "~" {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/")
    } else {
        partial.to_string()
    };

    let (parent_dir, prefix) = if expanded.ends_with('/') {
        // Browsing a directory: list its contents
        (expanded.as_str(), "")
    } else {
        // Partial name: list parent and filter
        let path = Path::new(&expanded);
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => (
                if parent.as_os_str().is_empty() {
                    "."
                } else {
                    // Leak is ugly but we need a &str with the right lifetime.
                    // In practice this is called rarely and the strings are small.
                    // Use a local String instead.
                    return filesystem_complete_inner(
                        &parent.to_string_lossy(),
                        &name.to_string_lossy(),
                    );
                },
                name.to_str().unwrap_or(""),
            ),
            _ => return Vec::new(),
        }
    };

    filesystem_complete_inner(parent_dir, prefix)
}

fn filesystem_complete_inner(dir: &str, prefix: &str) -> Vec<String> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };

    let prefix_lower = prefix.to_lowercase();
    let mut results = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden directories unless prefix starts with .
        if name_str.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }

        // Only directories
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        // For symlinks, check if target is a directory
        if file_type.is_symlink() {
            if let Ok(meta) = entry.metadata() {
                if !meta.is_dir() {
                    continue;
                }
            } else {
                continue;
            }
        }

        if prefix.is_empty() || name_str.to_lowercase().starts_with(&prefix_lower) {
            let full = if dir.ends_with('/') {
                format!("{dir}{name_str}")
            } else {
                format!("{dir}/{name_str}")
            };
            results.push(full);
        }
    }

    results.sort();
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projects_manager_add_and_get() {
        let mut mgr = ProjectsManager {
            config_file: PathBuf::from("/tmp/vibecraft-test-projects.json"),
            projects: Vec::new(),
        };

        mgr.add_project("/home/user/project-a", "Project A");
        // Bump last_used so ordering is deterministic even within the same ms
        mgr.projects[0].last_used = 1000;
        mgr.add_project("/home/user/project-b", "Project B");
        mgr.projects[1].last_used = 2000;

        let projects = mgr.get_projects();
        assert_eq!(projects.len(), 2);
        // Most recent first
        assert_eq!(projects[0].path, "/home/user/project-b");

        // Cleanup
        let _ = std::fs::remove_file("/tmp/vibecraft-test-projects.json");
    }

    #[test]
    fn test_projects_manager_update_existing() {
        let mut mgr = ProjectsManager {
            config_file: PathBuf::from("/tmp/vibecraft-test-projects2.json"),
            projects: Vec::new(),
        };

        mgr.add_project("/home/user/proj", "Proj");
        mgr.add_project("/home/user/proj", "Proj Updated");

        assert_eq!(mgr.projects.len(), 1);
        assert_eq!(mgr.projects[0].name, "Proj Updated");
        assert_eq!(mgr.projects[0].use_count, 2);

        let _ = std::fs::remove_file("/tmp/vibecraft-test-projects2.json");
    }

    #[test]
    fn test_projects_manager_remove() {
        let mut mgr = ProjectsManager {
            config_file: PathBuf::from("/tmp/vibecraft-test-projects3.json"),
            projects: Vec::new(),
        };

        mgr.add_project("/a", "A");
        mgr.add_project("/b", "B");
        mgr.remove_project("/a");

        assert_eq!(mgr.projects.len(), 1);
        assert_eq!(mgr.projects[0].path, "/b");

        let _ = std::fs::remove_file("/tmp/vibecraft-test-projects3.json");
    }

    #[test]
    fn test_autocomplete_known_projects() {
        let mut mgr = ProjectsManager {
            config_file: PathBuf::from("/tmp/vibecraft-test-projects4.json"),
            projects: Vec::new(),
        };

        mgr.add_project("/home/user/vibecraft", "Vibecraft");
        mgr.add_project("/home/user/other", "Other");

        let results = mgr.autocomplete("vibe", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "/home/user/vibecraft");

        let _ = std::fs::remove_file("/tmp/vibecraft-test-projects4.json");
    }

    #[test]
    fn test_filesystem_complete_tmp() {
        // /tmp should exist and have subdirectories
        let results = filesystem_complete("/tmp/");
        // We can't assert specific contents but it should not panic
        assert!(results.iter().all(|r| r.starts_with("/tmp/")));
    }
}
