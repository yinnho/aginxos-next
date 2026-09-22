//! A2A (Agent-to-Agent) Protocol — cross-framework agent interoperability.
//!
//! Google's A2A protocol enables cross-framework agent interoperability via
//! **Agent Cards** (JSON capability manifests) and **Task-based coordination**.

pub mod client;
pub mod types;

// Re-export all public types for backward compatibility
pub use client::{discover_external_agents, A2aClient};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a2a_task_status_transitions() {
        let task = A2aTask {
            id: "task-1".to_string(),
            session_id: None,
            status: A2aTaskStatus::Submitted.into(),
            messages: vec![],
        };
        assert_eq!(task.status, A2aTaskStatus::Submitted);

        let working = A2aTask {
            status: A2aTaskStatus::Working.into(),
            ..task.clone()
        };
        assert_eq!(working.status, A2aTaskStatus::Working);

        let completed = A2aTask {
            status: A2aTaskStatus::Completed.into(),
            ..task.clone()
        };
        assert_eq!(completed.status, A2aTaskStatus::Completed);

        let cancelled = A2aTask {
            status: A2aTaskStatus::Cancelled.into(),
            ..task.clone()
        };
        assert_eq!(cancelled.status, A2aTaskStatus::Cancelled);

        let failed = A2aTask {
            status: A2aTaskStatus::Failed.into(),
            ..task
        };
        assert_eq!(failed.status, A2aTaskStatus::Failed);
    }

    #[test]
    fn test_a2a_task_status_wrapper_object_form() {
        let json = r#"{"state":"completed","message":null}"#;
        let wrapper: A2aTaskStatusWrapper = serde_json::from_str(json).unwrap();
        assert_eq!(wrapper, A2aTaskStatus::Completed);
        assert_eq!(wrapper.state(), &A2aTaskStatus::Completed);

        let json_with_msg = r#"{"state":"working","message":{"text":"Processing..."}}"#;
        let wrapper2: A2aTaskStatusWrapper = serde_json::from_str(json_with_msg).unwrap();
        assert_eq!(wrapper2, A2aTaskStatus::Working);

        let json_bare = r#""completed""#;
        let wrapper3: A2aTaskStatusWrapper = serde_json::from_str(json_bare).unwrap();
        assert_eq!(wrapper3, A2aTaskStatus::Completed);
    }

    #[test]
    fn test_a2a_message_serde() {
        let msg = A2aMessage {
            role: "user".to_string(),
            parts: vec![
                A2aPart::Text {
                    text: "Hello".to_string(),
                },
                A2aPart::Data {
                    mime_type: "application/json".to_string(),
                    data: serde_json::json!({"key": "value"}),
                },
            ],
        };

        let json = serde_json::to_string(&msg).unwrap();
        let back: A2aMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "user");
        assert_eq!(back.parts.len(), 2);

        match &back.parts[0] {
            A2aPart::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("Expected Text part"),
        }
    }

    #[test]
    fn test_task_store_insert_and_get() {
        let store = A2aTaskStore::new(10);
        let task = A2aTask {
            id: "t-1".to_string(),
            session_id: None,
            status: A2aTaskStatus::Working.into(),
            messages: vec![],
        };
        store.insert(task);
        assert_eq!(store.len(), 1);

        let got = store.get("t-1").unwrap();
        assert_eq!(got.status, A2aTaskStatus::Working);
    }

    #[test]
    fn test_task_store_complete_and_fail() {
        let store = A2aTaskStore::new(10);
        let task = A2aTask {
            id: "t-2".to_string(),
            session_id: None,
            status: A2aTaskStatus::Working.into(),
            messages: vec![],
        };
        store.insert(task);

        store.complete(
            "t-2",
            A2aMessage {
                role: "agent".to_string(),
                parts: vec![A2aPart::Text {
                    text: "Done".to_string(),
                }],
            },
        );

        let completed = store.get("t-2").unwrap();
        assert_eq!(completed.status, A2aTaskStatus::Completed);
        assert_eq!(completed.messages.len(), 1);
    }

    #[test]
    fn test_task_store_cancel() {
        let store = A2aTaskStore::new(10);
        let task = A2aTask {
            id: "t-3".to_string(),
            session_id: None,
            status: A2aTaskStatus::Working.into(),
            messages: vec![],
        };
        store.insert(task);
        assert!(store.cancel("t-3"));
        assert_eq!(store.get("t-3").unwrap().status, A2aTaskStatus::Cancelled);
        assert!(!store.cancel("t-999"));
    }

    #[test]
    fn test_task_store_eviction() {
        let store = A2aTaskStore::new(2);
        for i in 0..2 {
            let task = A2aTask {
                id: format!("t-{i}"),
                session_id: None,
                status: A2aTaskStatus::Completed.into(),
                messages: vec![],
            };
            store.insert(task);
        }
        assert_eq!(store.len(), 2);

        let task = A2aTask {
            id: "t-2".to_string(),
            session_id: None,
            status: A2aTaskStatus::Working.into(),
            messages: vec![],
        };
        store.insert(task);
        assert!(store.len() <= 2);
    }

    #[test]
    fn test_a2a_config_serde() {
        use ::carrier_types::config::{A2aConfig, ExternalAgent};

        let config = A2aConfig {
            enabled: true,
            listen_path: "/a2a".to_string(),
            external_agents: vec![ExternalAgent {
                name: "other-agent".to_string(),
                url: "https://other.example.com".to_string(),
            }],
        };

        let json = serde_json::to_string(&config).unwrap();
        let back: A2aConfig = serde_json::from_str(&json).unwrap();
        assert!(back.enabled);
        assert_eq!(back.listen_path, "/a2a");
        assert_eq!(back.external_agents.len(), 1);
        assert_eq!(back.external_agents[0].name, "other-agent");
    }
}
