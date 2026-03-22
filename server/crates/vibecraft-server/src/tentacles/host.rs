use std::process::Output;

use tokio::process::Command;

/// Executes commands directly on the host machine.
#[derive(Debug, Clone)]
pub struct HostTarget {
    pub name: String,
}

impl HostTarget {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub async fn exec(&self, cmd: &[&str], timeout_secs: Option<u64>) -> anyhow::Result<Output> {
        let mut command = Command::new(cmd[0]);
        if cmd.len() > 1 {
            command.args(&cmd[1..]);
        }

        if let Some(t) = timeout_secs {
            tokio::time::timeout(std::time::Duration::from_secs(t), command.output())
                .await
                .map_err(|_| anyhow::anyhow!("Command timed out after {t}s"))?
                .map_err(Into::into)
        } else {
            command.output().await.map_err(Into::into)
        }
    }

    pub async fn exec_with_stdin(
        &self,
        cmd: &[&str],
        stdin_data: &[u8],
        timeout_secs: Option<u64>,
    ) -> anyhow::Result<Output> {
        use tokio::io::AsyncWriteExt;

        let mut command = Command::new(cmd[0]);
        if cmd.len() > 1 {
            command.args(&cmd[1..]);
        }

        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(stdin_data).await?;
        }

        if let Some(t) = timeout_secs {
            tokio::time::timeout(std::time::Duration::from_secs(t), child.wait_with_output())
                .await
                .map_err(|_| anyhow::anyhow!("Command timed out after {t}s"))?
                .map_err(Into::into)
        } else {
            child.wait_with_output().await.map_err(Into::into)
        }
    }
}
