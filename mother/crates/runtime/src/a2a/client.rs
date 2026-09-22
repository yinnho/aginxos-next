//! A2A Client — discover and interact with external A2A agents.

use super::types::{A2aTask, AgentCard};
use tracing::{debug, info, warn};
use carrier_types::error::{CarrierError, CarrierResult};

/// Discover all configured external A2A agents and return their cards.
///
/// Called during kernel boot to populate the list of known external agents.
pub async fn discover_external_agents(
    agents: &[carrier_types::config::ExternalAgent],
) -> Vec<(String, AgentCard)> {
    let client = A2aClient::new();
    let mut discovered = Vec::new();

    for agent in agents {
        match client.discover(&agent.url).await {
            Ok(card) => {
                info!(
                    name = %agent.name,
                    url = %agent.url,
                    skills = card.skills.len(),
                    "Discovered external A2A agent"
                );
                discovered.push((agent.name.clone(), card));
            }
            Err(e) => {
                warn!(
                    name = %agent.name,
                    url = %agent.url,
                    error = %e,
                    "Failed to discover external A2A agent"
                );
            }
        }
    }

    if !discovered.is_empty() {
        info!("A2A: discovered {} external agent(s)", discovered.len());
    }

    discovered
}

/// Client for discovering and interacting with external A2A agents.
pub struct A2aClient {
    client: reqwest::Client,
}

impl A2aClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    pub async fn discover(&self, url: &str) -> CarrierResult<AgentCard> {
        let agent_json_url = format!("{}/.well-known/agent.json", url.trim_end_matches('/'));

        debug!(url = %agent_json_url, "Discovering A2A agent");

        let response = self
            .client
            .get(&agent_json_url)
            .header(
                "User-Agent",
                format!("OpenCarrier/{} A2A", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("A2A discovery failed: {e}")))?;

        if !response.status().is_success() {
            return Err(CarrierError::Network(format!(
                "A2A discovery returned {}",
                response.status()
            )));
        }

        let card: AgentCard = response
            .json()
            .await
            .map_err(|e| CarrierError::Serialization(format!("Invalid Agent Card: {e}")))?;

        info!(agent = %card.name, skills = card.skills.len(), "Discovered A2A agent");
        Ok(card)
    }

    pub async fn send_task(
        &self,
        url: &str,
        message: &str,
        session_id: Option<&str>,
    ) -> CarrierResult<A2aTask> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tasks/send",
            "params": {
                "message": {
                    "role": "user",
                    "parts": [{"type": "text", "text": message}]
                },
                "sessionId": session_id,
            }
        });

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("A2A send_task failed: {e}")))?;

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CarrierError::Serialization(format!("Invalid A2A response: {e}")))?;

        if let Some(result) = body.get("result") {
            serde_json::from_value(result.clone())
                .map_err(|e| CarrierError::Serialization(format!("Invalid A2A task response: {e}")))
        } else if let Some(error) = body.get("error") {
            Err(CarrierError::Network(format!("A2A error: {}", error)))
        } else {
            Err(CarrierError::Serialization("Empty A2A response".into()))
        }
    }

    pub async fn get_task(&self, url: &str, task_id: &str) -> CarrierResult<A2aTask> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tasks/get",
            "params": {
                "id": task_id,
            }
        });

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("A2A get_task failed: {e}")))?;

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CarrierError::Serialization(format!("Invalid A2A response: {e}")))?;

        if let Some(result) = body.get("result") {
            serde_json::from_value(result.clone())
                .map_err(|e| CarrierError::Serialization(format!("Invalid A2A task: {e}")))
        } else {
            Err(CarrierError::Serialization("Empty A2A response".into()))
        }
    }
}

impl Default for A2aClient {
    fn default() -> Self {
        Self::new()
    }
}
