use crate::error::{NexusError, Result};
use std::process::Command;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct DockerResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub container_id: String,
}

pub struct DockerSandbox {
    image: String,
}

impl DockerSandbox {
    pub fn new() -> Self {
        Self {
            image: "nexus-sandbox:latest".to_string(),
        }
    }

    pub async fn execute(&self, command: &str, working_dir: &Path) -> Result<DockerResult> {
        // Check if Docker is available
        if !self.is_docker_available() {
            return Err(NexusError::Configuration(
                "Docker is not available. Please install Docker to use the Shadow Run feature.".to_string()
            ));
        }

        // Ensure the sandbox image exists (build if needed)
        self.ensure_image().await?;

        // Generate a unique container name
        let container_id = format!("nexus-sandbox-{}", uuid::Uuid::new_v4());
        
        let start = Instant::now();

        // Build Docker run command
        let mut docker_cmd = Command::new("docker");
        docker_cmd
            .arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(&container_id)
            .arg("-v")
            .arg(format!("{}:/workspace", working_dir.display()))
            .arg("-w")
            .arg("/workspace")
            .arg("--network")
            .arg("none")  // Isolate network by default for security
            .arg(&self.image)
            .arg("sh")
            .arg("-c")
            .arg(command);

        let output = docker_cmd.output()
            .map_err(|e| NexusError::Io(e))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(DockerResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms,
            container_id,
        })
    }

    pub async fn execute_with_network(&self, command: &str, working_dir: &Path) -> Result<DockerResult> {
        // Similar to execute but with network enabled for package managers
        if !self.is_docker_available() {
            return Err(NexusError::Configuration(
                "Docker is not available. Please install Docker to use the Shadow Run feature.".to_string()
            ));
        }

        // Ensure the sandbox image exists (build if needed)
        self.ensure_image().await?;

        let container_id = format!("nexus-sandbox-{}", uuid::Uuid::new_v4());
        let start = Instant::now();

        let mut docker_cmd = Command::new("docker");
        docker_cmd
            .arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(&container_id)
            .arg("-v")
            .arg(format!("{}:/workspace", working_dir.display()))
            .arg("-w")
            .arg("/workspace")
            .arg(&self.image)
            .arg("sh")
            .arg("-c")
            .arg(command);

        let output = docker_cmd.output()
            .map_err(|e| NexusError::Io(e))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(DockerResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms,
            container_id,
        })
    }

    pub async fn ensure_image(&self) -> Result<()> {
        // Check if the sandbox image exists
        let output = Command::new("docker")
            .args(&["images", "-q", &self.image])
            .output()
            .map_err(|e| NexusError::Io(e))?;

        let image_exists = !output.stdout.is_empty();

        if !image_exists {
            println!("Sandbox image not found. Building...");
            self.build_image().await?;
        }

        Ok(())
    }

    async fn build_image(&self) -> Result<()> {
        // Create a minimal Dockerfile for the sandbox
        let dockerfile = r#"
FROM alpine:latest
RUN apk add --no-cache bash curl nodejs npm python3 py3-pip git
WORKDIR /workspace
CMD ["sh"]
"#;

        // Write Dockerfile to temp location
        let temp_dir = std::env::temp_dir().join("nexus-sandbox-build");
        std::fs::create_dir_all(&temp_dir)?;
        std::fs::write(temp_dir.join("Dockerfile"), dockerfile)?;

        // Build the image
        let output = Command::new("docker")
            .args(&["build", "-t", &self.image, temp_dir.to_str().unwrap()])
            .output()
            .map_err(|e| NexusError::Io(e))?;

        if !output.status.success() {
            return Err(NexusError::Configuration(format!(
                "Failed to build sandbox image: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        println!("✓ Sandbox image built successfully");
        Ok(())
    }

    fn is_docker_available(&self) -> bool {
        Command::new("docker")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub async fn cleanup(&self, container_id: &str) -> Result<()> {
        // Force remove container if it's still running
        let _ = Command::new("docker")
            .args(&["rm", "-f", container_id])
            .output();

        Ok(())
    }
}
