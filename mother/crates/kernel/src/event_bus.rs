//! Event bus — pub/sub with pattern matching and history ring buffer.

use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::debug;
use carrier_types::agent::AgentId;
use carrier_types::event::{Event, EventTarget};

/// Maximum events retained in the history ring buffer.
const HISTORY_SIZE: usize = 1000;

/// The central event bus for inter-agent and system communication.
pub struct EventBus {
    /// Broadcast channel for all events.
    sender: broadcast::Sender<Event>,
    /// Per-agent event channels.
    agent_channels: DashMap<AgentId, broadcast::Sender<Event>>,
    /// Event history ring buffer.
    history: Arc<RwLock<VecDeque<Event>>>,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            sender,
            agent_channels: DashMap::new(),
            history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_SIZE))),
        }
    }

    /// Publish an event to the bus.
    pub async fn publish(&self, event: Event) {
        debug!(
            event_id = %event.id,
            source = %event.source,
            "Publishing event"
        );

        // Store in history
        {
            let mut history = self.history.write().await;
            if history.len() >= HISTORY_SIZE {
                history.pop_front();
            }
            history.push_back(event.clone());
        }

        // Route to target
        match &event.target {
            EventTarget::Agent(agent_id) => {
                if let Some(sender) = self.agent_channels.get(agent_id) {
                    let _ = sender.send(event.clone());
                }
            }
            EventTarget::Broadcast => {
                let _ = self.sender.send(event.clone());
                for entry in self.agent_channels.iter() {
                    let _ = entry.value().send(event.clone());
                }
            }
            EventTarget::Pattern(_pattern) => {
                // Phase 1: broadcast to all for pattern matching
                let _ = self.sender.send(event.clone());
            }
            EventTarget::System => {
                let _ = self.sender.send(event.clone());
            }
        }
    }

    /// Get recent event history.
    pub async fn history(&self, limit: usize) -> Vec<Event> {
        let history = self.history.read().await;
        history.iter().rev().take(limit).cloned().collect()
    }

    /// Remove an agent's channel when it's terminated.
    pub fn unsubscribe_agent(&self, agent_id: AgentId) {
        self.agent_channels.remove(&agent_id);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrier_types::event::{EventPayload, SystemEvent};

    #[tokio::test]
    async fn test_publish_and_history() {
        let bus = EventBus::new();
        let agent_id = AgentId::new();
        let event = Event::new(
            agent_id,
            EventTarget::System,
            EventPayload::System(SystemEvent::KernelStarted),
        );
        bus.publish(event).await;
        let history = bus.history(10).await;
        assert_eq!(history.len(), 1);
    }
}
