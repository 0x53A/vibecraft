use std::process::Output;

use tokio::process::Command;

/// Executes commands inside a Docker container via `docker exec`.
#[derive(Debug, Clone)]
pub struct DockerTarget {
    pub name: String,
    pub container: String,
    /// Set for targets built from a Dockerfile (needed for rebuild).
    pub dockerfile: Option<String>,
    pub volumes: Vec<String>,
    pub image_tag: Option<String>,
}

impl DockerTarget {
    pub fn new(name: String, container: String) -> Self {
        Self {
            name,
            container,
            dockerfile: None,
            volumes: Vec::new(),
            image_tag: None,
        }
    }

    /// Create a container from an image with optional volume mounts.
    pub async fn create_from_image(
        name: String,
        container_name: String,
        image: &str,
        volumes: &[String],
    ) -> anyhow::Result<Self> {
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            container_name.clone(),
        ];
        for vol in volumes {
            args.push("-v".to_string());
            args.push(vol.to_string());
        }
        args.push(image.to_string());
        args.push("sleep".to_string());
        args.push("infinity".to_string());

        let output = Command::new("docker").args(&args).output().await?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create container from image {image}: {err}");
        }
        tracing::info!("Created container {container_name} from image {image}");
        Ok(Self {
            name,
            container: container_name,
            dockerfile: None,
            volumes: volumes.to_vec(),
            image_tag: Some(image.to_string()),
        })
    }

    /// Build image from Dockerfile and create a container from it.
    pub async fn create_from_dockerfile(
        name: String,
        container_name: String,
        dockerfile: &str,
        volumes: &[String],
    ) -> anyhow::Result<Self> {
        let tag = format!("tentacles-{container_name}");
        let context = std::path::Path::new(dockerfile)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let output = Command::new("docker")
            .args(["build", "-f", dockerfile, "-t", &tag, &context])
            .output()
            .await?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Docker build failed: {err}");
        }
        tracing::info!("Built image {tag} from {dockerfile}");

        // Create container
        let mut target =
            Self::create_from_image(name, container_name, &tag, volumes).await?;
        target.dockerfile = Some(dockerfile.to_string());
        target.image_tag = Some(tag);
        Ok(target)
    }

    /// Rebuild: stop+remove old container, rebuild image from Dockerfile, create new container.
    /// Returns a new DockerTarget with the updated container name.
    pub async fn rebuild(&self) -> anyhow::Result<(Self, String)> {
        let dockerfile = self
            .dockerfile
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Cannot rebuild: no Dockerfile associated with this target"))?;
        let tag = self
            .image_tag
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Cannot rebuild: no image tag"))?;

        // Stop and remove old container
        let _ = Command::new("docker")
            .args(["stop", &self.container])
            .output()
            .await;
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output()
            .await;

        // Rebuild image using Dockerfile's parent as context
        let context = std::path::Path::new(dockerfile)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let build_output = Command::new("docker")
            .args(["build", "-f", dockerfile, "-t", tag, &context])
            .output()
            .await?;

        let build_log = format!(
            "{}{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr),
        );

        if !build_output.status.success() {
            anyhow::bail!("Docker build failed:\n{build_log}");
        }
        tracing::info!("Rebuilt image {tag} from {dockerfile}");

        // Create new container with fresh name
        let new_container_name = format!(
            "tentacles-{}-{}",
            self.name,
            &uuid::Uuid::new_v4().to_string()[..8]
        );
        let mut target =
            Self::create_from_image(self.name.clone(), new_container_name, tag, &self.volumes)
                .await?;
        target.dockerfile = Some(dockerfile.to_string());
        target.image_tag = Some(tag.to_string());

        Ok((target, build_log))
    }

    /// Ensure the target container is running.
    pub async fn ensure_running(&self) -> anyhow::Result<()> {
        let output = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", &self.container])
            .output()
            .await?;

        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if state == "true" {
            tracing::info!("Container {} is running", self.container);
            return Ok(());
        }

        if output.status.success() {
            tracing::info!("Container {} exists but stopped, starting...", self.container);
            let start = Command::new("docker")
                .args(["start", &self.container])
                .output()
                .await?;
            if !start.status.success() {
                anyhow::bail!(
                    "Failed to start container {}: {}",
                    self.container,
                    String::from_utf8_lossy(&start.stderr)
                );
            }
            return Ok(());
        }

        anyhow::bail!(
            "Container {} not found. Create it first:\n  \
             docker run -d --name {} ubuntu:24.04 sleep infinity",
            self.container,
            self.container
        )
    }

    pub async fn exec(&self, cmd: &[&str], timeout_secs: Option<u64>) -> anyhow::Result<Output> {
        let mut args = vec!["exec".to_string(), self.container.clone()];
        for c in cmd {
            args.push(c.to_string());
        }

        let mut command = Command::new("docker");
        command.args(&args);

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

        let mut args = vec!["exec", "-i", &self.container];
        for c in cmd {
            args.push(c);
        }

        let mut child = Command::new("docker")
            .args(&args)
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
