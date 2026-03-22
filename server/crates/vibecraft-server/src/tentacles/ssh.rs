use std::process::Output;

/// SSH target (placeholder — not yet implemented).
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub name: String,
    pub host: String,
}

impl SshTarget {
    pub fn new(name: String, host: String) -> Self {
        Self { name, host }
    }

    pub async fn exec(&self, _cmd: &[&str], _timeout_secs: Option<u64>) -> anyhow::Result<Output> {
        anyhow::bail!("SSH target '{}' (host: {}) is not yet implemented", self.name, self.host)
    }

    pub async fn exec_with_stdin(
        &self,
        _cmd: &[&str],
        _stdin_data: &[u8],
        _timeout_secs: Option<u64>,
    ) -> anyhow::Result<Output> {
        anyhow::bail!("SSH target '{}' (host: {}) is not yet implemented", self.name, self.host)
    }
}
