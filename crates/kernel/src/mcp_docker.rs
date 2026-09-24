//! Docker lifecycle management for MCP servers.
//!
//! Generates docker-compose.yml from .mcp.json manifests and provides
//! start/stop/restart/status/logs operations via `docker compose`.

use std::path::Path;
use std::process::Stdio;
use tracing::info;

/// Container state returned by `status()`.
#[derive(Debug, serde::Serialize)]
pub struct ContainerState {
    pub name: String,
    pub running: bool,
    pub status: String,
    pub ports: String,
    pub uptime: String,
}

/// Run `docker compose` command in the server directory.
async fn compose_cmd(dir: &Path, args: &[&str]) -> Result<String, String> {
    // Check docker availability
    let docker_check = tokio::process::Command::new("docker")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Docker not available: {e}"))?;

    if !docker_check.status.success() {
        return Err("Docker is not installed or not running".to_string());
    }

    let output = tokio::process::Command::new("docker")
        .arg("compose")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("docker compose failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        Err(format!("docker compose {}: {stderr}", args.join(" ")))
    } else {
        Ok(stdout)
    }
}

/// Start an MCP server's Docker container.
pub async fn start(dir: &Path) -> Result<ContainerState, String> {
    info!("Starting Docker container...");
    compose_cmd(dir, &["up", "-d"]).await?;
    status(dir).await
}

/// Stop an MCP server's Docker container.
pub async fn stop(dir: &Path) -> Result<(), String> {
    info!("Stopping Docker container...");
    compose_cmd(dir, &["down"]).await.map(|_| ())
}

/// Restart an MCP server's Docker container.
pub async fn restart(dir: &Path) -> Result<ContainerState, String> {
    info!("Restarting Docker container...");
    compose_cmd(dir, &["restart"]).await?;
    status(dir).await
}

/// Get the status of an MCP server's Docker container.
pub async fn status(dir: &Path) -> Result<ContainerState, String> {
    let output = compose_cmd(dir, &["ps", "--format", "json"]).await?;

    // docker compose ps --format json returns one JSON object per line
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let running = v["State"].as_str() == Some("running");
            return Ok(ContainerState {
                name: v["Name"].as_str().unwrap_or("unknown").to_string(),
                running,
                status: v["Status"].as_str().unwrap_or("unknown").to_string(),
                ports: v["Ports"].as_str().unwrap_or("").to_string(),
                uptime: {
                    let running_for = v["RunningFor"].as_str().unwrap_or("");
                    if running_for.is_empty() {
                        v["CreatedAt"].as_str().unwrap_or("").to_string()
                    } else {
                        running_for.to_string()
                    }
                },
            });
        }
    }

    Ok(ContainerState {
        name: String::new(),
        running: false,
        status: "not running".to_string(),
        ports: String::new(),
        uptime: String::new(),
    })
}

/// Get logs from an MCP server's Docker container.
pub async fn logs(dir: &Path, tail: usize) -> Result<String, String> {
    compose_cmd(dir, &["logs", "--tail", &tail.to_string()]).await
}
