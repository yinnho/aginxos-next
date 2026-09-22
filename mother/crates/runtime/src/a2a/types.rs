//! A2A data types — Agent Cards, Tasks, Messages, and Task Store.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// A2A Agent Card
// ---------------------------------------------------------------------------

/// A2A Agent Card — describes an agent's capabilities to external systems.
///
/// Served at `/.well-known/agent.json` per the A2A specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: String,
    pub version: String,
    pub capabilities: AgentCapabilities,
    pub skills: Vec<AgentSkill>,
    #[serde(default)]
    pub default_input_modes: Vec<String>,
    #[serde(default)]
    pub default_output_modes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub push_notifications: bool,
    pub state_transition_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub examples: Vec<String>,
}

// ---------------------------------------------------------------------------
// A2A Task
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aTask {
    pub id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub status: A2aTaskStatusWrapper,
    #[serde(default)]
    pub messages: Vec<A2aMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum A2aTaskStatus {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum A2aTaskStatusWrapper {
    Object {
        state: A2aTaskStatus,
        #[serde(default)]
        message: Option<serde_json::Value>,
    },
    Enum(A2aTaskStatus),
}

impl A2aTaskStatusWrapper {
    pub fn state(&self) -> &A2aTaskStatus {
        match self {
            Self::Object { state, .. } => state,
            Self::Enum(s) => s,
        }
    }
}

impl From<A2aTaskStatus> for A2aTaskStatusWrapper {
    fn from(status: A2aTaskStatus) -> Self {
        Self::Enum(status)
    }
}

impl PartialEq<A2aTaskStatus> for A2aTaskStatusWrapper {
    fn eq(&self, other: &A2aTaskStatus) -> bool {
        self.state() == other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aMessage {
    pub role: String,
    pub parts: Vec<A2aPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum A2aPart {
    Text {
        text: String,
    },
    File {
        name: String,
        mime_type: String,
        data: String,
    },
    Data {
        mime_type: String,
        data: serde_json::Value,
    },
}

// ---------------------------------------------------------------------------
// A2A Task Store
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct A2aTaskStore {
    tasks: Mutex<HashMap<String, A2aTask>>,
    max_tasks: usize,
}

impl A2aTaskStore {
    pub fn new(max_tasks: usize) -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            max_tasks,
        }
    }

    pub fn insert(&self, task: A2aTask) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if tasks.len() >= self.max_tasks {
            let evict_key = tasks
                .iter()
                .filter(|(_, t)| {
                    matches!(
                        t.status.state(),
                        A2aTaskStatus::Completed | A2aTaskStatus::Failed | A2aTaskStatus::Cancelled
                    )
                })
                .map(|(k, _)| k.clone())
                .next();
            if let Some(key) = evict_key {
                tasks.remove(&key);
            }
        }
        tasks.insert(task.id.clone(), task);
    }

    pub fn get(&self, task_id: &str) -> Option<A2aTask> {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
            .cloned()
    }

    pub fn update_status(&self, task_id: &str, status: A2aTaskStatus) -> bool {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = tasks.get_mut(task_id) {
            task.status = status.into();
            true
        } else {
            false
        }
    }

    pub fn complete(&self, task_id: &str, response: A2aMessage) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = tasks.get_mut(task_id) {
            task.messages.push(response);
            task.status = A2aTaskStatus::Completed.into();
        }
    }

    pub fn fail(&self, task_id: &str, error_message: A2aMessage) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = tasks.get_mut(task_id) {
            task.messages.push(error_message);
            task.status = A2aTaskStatus::Failed.into();
        }
    }

    pub fn cancel(&self, task_id: &str) -> bool {
        self.update_status(task_id, A2aTaskStatus::Cancelled)
    }

    pub fn len(&self) -> usize {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for A2aTaskStore {
    fn default() -> Self {
        Self::new(1000)
    }
}
