use std::collections::HashMap;
use std::process::Output;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::docker::DockerTarget;
use super::host::HostTarget;
use super::ssh::SshTarget;

/// Concrete target enum — no trait objects needed.
#[derive(Debug, Clone)]
pub enum Target {
    Host(HostTarget),
    Docker(DockerTarget),
    Ssh(SshTarget),
}

impl Target {
    pub fn name(&self) -> &str {
        match self {
            Self::Host(t) => &t.name,
            Self::Docker(t) => &t.name,
            Self::Ssh(t) => &t.name,
        }
    }

    pub fn target_type(&self) -> &str {
        match self {
            Self::Host(_) => "host",
            Self::Docker(_) => "container",
            Self::Ssh(_) => "ssh",
        }
    }

    pub async fn exec(&self, cmd: &[&str], timeout_secs: Option<u64>) -> anyhow::Result<Output> {
        match self {
            Self::Host(t) => t.exec(cmd, timeout_secs).await,
            Self::Docker(t) => t.exec(cmd, timeout_secs).await,
            Self::Ssh(t) => t.exec(cmd, timeout_secs).await,
        }
    }

    pub async fn exec_with_stdin(
        &self,
        cmd: &[&str],
        stdin_data: &[u8],
        timeout_secs: Option<u64>,
    ) -> anyhow::Result<Output> {
        match self {
            Self::Host(t) => t.exec_with_stdin(cmd, stdin_data, timeout_secs).await,
            Self::Docker(t) => t.exec_with_stdin(cmd, stdin_data, timeout_secs).await,
            Self::Ssh(t) => t.exec_with_stdin(cmd, stdin_data, timeout_secs).await,
        }
    }
}

/// Serializable target info for API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub name: String,
    pub target_type: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
}

/// Thread-safe registry of named targets with runtime add/remove.
#[derive(Clone)]
pub struct TargetRegistry {
    targets: Arc<RwLock<HashMap<String, Arc<Target>>>>,
    infos: Arc<RwLock<HashMap<String, TargetInfo>>>,
}

impl std::fmt::Debug for TargetRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetRegistry").finish_non_exhaustive()
    }
}

impl TargetRegistry {
    pub fn new() -> Self {
        Self {
            targets: Arc::new(RwLock::new(HashMap::new())),
            infos: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add(&self, info: TargetInfo, target: Arc<Target>) {
        let name = info.name.clone();
        self.infos.write().await.insert(name.clone(), info);
        self.targets.write().await.insert(name, target);
    }

    pub async fn remove(&self, name: &str) -> bool {
        let removed = self.targets.write().await.remove(name).is_some();
        self.infos.write().await.remove(name);
        removed
    }

    pub async fn get(&self, name: &str) -> Option<Arc<Target>> {
        self.targets.read().await.get(name).cloned()
    }

    pub async fn get_info(&self, name: &str) -> Option<TargetInfo> {
        self.infos.read().await.get(name).cloned()
    }

    pub async fn list(&self) -> Vec<TargetInfo> {
        self.infos.read().await.values().cloned().collect()
    }

    pub async fn names(&self) -> Vec<String> {
        self.targets.read().await.keys().cloned().collect()
    }
}
