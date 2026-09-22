//! Process supervision — graceful shutdown, signal handling, and health monitoring.

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;
use tracing::{info, warn};

/// Shutdown signal manager with health monitoring.
pub struct Supervisor {
    /// Send side of the shutdown signal.
    shutdown_tx: watch::Sender<bool>,
    /// Receive side of the shutdown signal (clonable).
    shutdown_rx: watch::Receiver<bool>,
    /// Restart count (how many times agents have been restarted).
    restart_count: AtomicU64,
    /// Total panics caught across all agents.
    panic_count: AtomicU64,
}

impl Supervisor {
    /// Create a new supervisor.
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            shutdown_tx: tx,
            shutdown_rx: rx,
            restart_count: AtomicU64::new(0),
            panic_count: AtomicU64::new(0),
        }
    }

    /// Get a receiver that will be notified on shutdown.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    /// Trigger a graceful shutdown.
    pub fn shutdown(&self) {
        info!("Supervisor: initiating graceful shutdown");
        let _ = self.shutdown_tx.send(true);
    }

    /// Check if shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown_rx.borrow()
    }

    /// Record that a panic was caught during agent execution.
    pub fn record_panic(&self) {
        self.panic_count.fetch_add(1, Ordering::Relaxed);
        warn!(
            total_panics = self.panic_count.load(Ordering::Relaxed),
            "Agent panic recorded"
        );
    }

    /// Get the total number of panics caught.
    pub fn panic_count(&self) -> u64 {
        self.panic_count.load(Ordering::Relaxed)
    }

    /// Get the total number of restarts.
    pub fn restart_count(&self) -> u64 {
        self.restart_count.load(Ordering::Relaxed)
    }

    /// Get a health summary.
    pub fn health(&self) -> SupervisorHealth {
        SupervisorHealth {
            is_shutting_down: self.is_shutting_down(),
            panic_count: self.panic_count(),
            restart_count: self.restart_count(),
        }
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

/// Health report from the supervisor.
#[derive(Debug, Clone)]
pub struct SupervisorHealth {
    pub is_shutting_down: bool,
    pub panic_count: u64,
    pub restart_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown() {
        let supervisor = Supervisor::new();
        assert!(!supervisor.is_shutting_down());
        supervisor.shutdown();
        assert!(supervisor.is_shutting_down());
    }

    #[test]
    fn test_subscribe() {
        let supervisor = Supervisor::new();
        let rx = supervisor.subscribe();
        assert!(!*rx.borrow());
        supervisor.shutdown();
        assert!(rx.has_changed().unwrap());
    }

    #[test]
    fn test_panic_tracking() {
        let supervisor = Supervisor::new();
        assert_eq!(supervisor.panic_count(), 0);
        supervisor.record_panic();
        supervisor.record_panic();
        assert_eq!(supervisor.panic_count(), 2);
    }

    #[test]
    fn test_health() {
        let supervisor = Supervisor::new();
        let health = supervisor.health();
        assert!(!health.is_shutting_down);
        assert_eq!(health.panic_count, 0);
        assert_eq!(health.restart_count, 0);
    }
}
