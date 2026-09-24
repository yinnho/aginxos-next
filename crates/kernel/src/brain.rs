//! Brain — the carrier's independent LLM brain.
//!
//! Single-layer architecture: one shared aginxbrain driver, routed by
//! modality name (sent as the `model` field). Health tracking is per-
//! modality so the circuit breaker can still take an individual modality
//! out of rotation if its upstream stalls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use dashmap::DashMap;
use carrier_runtime::drivers;
use carrier_runtime::llm_driver::{Brain as BrainTrait, DriverConfig, LlmDriver};
use tracing::{debug, info, warn};
use carrier_types::brain::{
    BrainConfig, BrainStatus, EndpointHealth, EndpointReport, ModalityInfo, ResolvedEndpoint,
};

// ---------------------------------------------------------------------------
// Per-modality health tracker (lock-free atomics)
// ---------------------------------------------------------------------------

/// Consecutive failures before circuit opens (modality is taken out of rotation).
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
/// How long to wait before allowing a probe request (half-open state).
const CIRCUIT_BREAKER_COOLDOWN_MS: u64 = 60_000; // 60s

/// Thread-safe health tracker for a single modality.
struct EndpointTracker {
    success_count: AtomicU64,
    failure_count: AtomicU64,
    total_latency_ms: AtomicU64,
    latency_count: AtomicU64,
    consecutive_failures: AtomicU32,
    /// Timestamp (ms since epoch) of the last failure. Used for circuit-breaker cooldown.
    last_failure_at: AtomicU64,
}

impl EndpointTracker {
    fn new() -> Self {
        Self {
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            consecutive_failures: AtomicU32::new(0),
            last_failure_at: AtomicU64::new(0),
        }
    }

    fn record_success(&self, latency_ms: u64) {
        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        if latency_ms > 0 {
            self.total_latency_ms
                .fetch_add(latency_ms, Ordering::Relaxed);
            self.latency_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_failure(&self, latency_ms: u64) {
        self.failure_count.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        if latency_ms > 0 {
            self.total_latency_ms
                .fetch_add(latency_ms, Ordering::Relaxed);
            self.latency_count.fetch_add(1, Ordering::Relaxed);
        }
        self.last_failure_at.store(now_ms(), Ordering::Relaxed);
    }

    /// Check if the circuit is open (modality should be skipped).
    /// Returns true if the modality is available for requests.
    fn is_available(&self) -> bool {
        let consec = self.consecutive_failures.load(Ordering::Relaxed);
        if consec < CIRCUIT_BREAKER_THRESHOLD {
            return true;
        }
        // Circuit is open — check if cooldown has passed (half-open)
        let last = self.last_failure_at.load(Ordering::Relaxed);
        let elapsed = now_ms().saturating_sub(last);
        elapsed >= CIRCUIT_BREAKER_COOLDOWN_MS
    }

    fn snapshot(&self) -> EndpointSnapshot {
        let success = self.success_count.load(Ordering::Relaxed);
        let failure = self.failure_count.load(Ordering::Relaxed);
        let total_lat = self.total_latency_ms.load(Ordering::Relaxed);
        let lat_count = self.latency_count.load(Ordering::Relaxed);
        let avg = total_lat.checked_div(lat_count).unwrap_or(0);
        let consec = self.consecutive_failures.load(Ordering::Relaxed);
        let circuit_open = consec >= CIRCUIT_BREAKER_THRESHOLD && !self.is_available();
        EndpointSnapshot {
            success,
            failure,
            avg_latency: avg,
            consecutive_failures: consec,
            circuit_open,
        }
    }
}

struct EndpointSnapshot {
    success: u64,
    failure: u64,
    avg_latency: u64,
    consecutive_failures: u32,
    circuit_open: bool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Brain
// ---------------------------------------------------------------------------

/// Provider name for the single aginxbrain backend.
const PROVIDER_NAME: &str = "aginxbrain";

/// The carrier's brain — a single shared driver, routed by modality.
pub struct Brain {
    config: BrainConfig,
    /// The single shared driver (all modalities use the same base_url + api_key).
    driver: Option<Arc<dyn LlmDriver>>,
    /// Per-modality health tracking. Key = modality name.
    health: DashMap<String, EndpointTracker>,
}

impl Brain {
    /// Create a new Brain from config. The single driver is created eagerly.
    pub fn new(config: BrainConfig) -> Result<Self, BrainError> {
        // Resolve API key: env first (incl. ~/.aginx/carrier/.env), then the
        // agsecretd sidecar leg (M36 — DECISIONS §9 on the OS side).
        let api_key = if config.api_key_env.is_empty() {
            None
        } else {
            carrier_types::env::get_secret(&config.api_key_env)
        };

        let driver_config = DriverConfig {
            api_key,
            base_url: Some(config.base_url.clone()),
        };

        let driver = match drivers::create_driver(&driver_config) {
            Ok(d) => Some(d),
            Err(e) => {
                warn!(
                    error = %e,
                    base_url = %config.base_url,
                    "Failed to create brain driver — all modality calls will fail"
                );
                None
            }
        };

        info!(
            modalities = config.modalities.len(),
            default_modality = %config.default_modality,
            driver_ready = driver.is_some(),
            "Brain initialized"
        );

        Ok(Self {
            config,
            driver,
            health: DashMap::new(),
        })
    }

    /// Test-only: build a Brain around an injected driver (fake driver in
    /// unit tests — no real aginxbrain call, `new()`'s eager driver creation
    /// is bypassed).
    #[cfg(test)]
    pub(crate) fn with_test_driver(config: BrainConfig, driver: Arc<dyn LlmDriver>) -> Self {
        Self {
            config,
            driver: Some(driver),
            health: DashMap::new(),
        }
    }

    // ── Query interface ─────────────────────────────────────

    /// List all available modalities with descriptions.
    pub fn list_modalities(&self) -> Vec<ModalityInfo> {
        self.config
            .modalities
            .iter()
            .map(|(name, me)| ModalityInfo {
                name: name.clone(),
                description: me.description.clone(),
            })
            .collect()
    }

    /// Get the resolved endpoint for a modality.
    ///
    /// Single-layer model: exactly one endpoint (the shared aginxbrain URL).
    /// Returns an empty vec if the modality is unknown or circuit-broken.
    /// Kept as a Vec so the existing fallback-iteration code in
    /// call_with_fallback / Brain::complete() works unchanged.
    pub fn endpoints_for(&self, modality: &str) -> Vec<ResolvedEndpoint> {
        // Resolve the modality (fall back to default_modality).
        let resolved = if self.config.modalities.contains_key(modality) {
            modality
        } else if self
            .config
            .modalities
            .contains_key(&self.config.default_modality)
        {
            &self.config.default_modality
        } else {
            return vec![];
        };

        // Driver must exist.
        if self.driver.is_none() {
            debug!(modality = %resolved, "Brain driver not ready, no endpoints");
            return vec![];
        }

        // Circuit-breaker: skip modalities with too many consecutive failures.
        if let Some(tracker) = self.health.get(resolved) {
            if !tracker.is_available() {
                warn!(
                    modality = %resolved,
                    consecutive = tracker.consecutive_failures.load(Ordering::Relaxed),
                    "Modality circuit-broken, skipping"
                );
                return vec![];
            }
        }

        // The modality name IS the model routing tag sent to aginxbrain.
        vec![ResolvedEndpoint {
            id: resolved.to_string(),
            model: resolved.to_string(),
            provider: PROVIDER_NAME.to_string(),
        }]
    }

    /// Get the shared driver. `endpoint_id` is the modality name but the
    /// driver is the same for all modalities.
    pub fn driver_for_endpoint(&self, _endpoint_id: &str) -> Option<Arc<dyn LlmDriver>> {
        self.driver.clone()
    }

    /// Report the result of a modality call. Non-blocking.
    pub fn report(&self, report: EndpointReport) {
        let tracker = self
            .health
            .entry(report.endpoint_id)
            .or_insert_with(EndpointTracker::new);

        if report.success {
            tracker.record_success(report.latency_ms);
        } else {
            tracker.record_failure(report.latency_ms);
        }
    }

    /// Get current Brain status snapshot.
    pub fn status(&self) -> BrainStatus {
        let modalities = self.list_modalities();

        let endpoints: Vec<EndpointHealth> = self
            .config
            .modalities
            .keys()
            .map(|name| {
                let snap = self
                    .health
                    .get(name)
                    .map(|t| t.snapshot())
                    .unwrap_or_else(|| EndpointSnapshot {
                        success: 0,
                        failure: 0,
                        avg_latency: 0,
                        consecutive_failures: 0,
                        circuit_open: false,
                    });

                EndpointHealth {
                    endpoint: name.clone(),
                    provider: PROVIDER_NAME.to_string(),
                    model: name.clone(),
                    driver_ready: self.driver.is_some(),
                    success_count: snap.success,
                    failure_count: snap.failure,
                    avg_latency_ms: snap.avg_latency,
                    consecutive_failures: snap.consecutive_failures,
                    circuit_open: snap.circuit_open,
                }
            })
            .collect();

        let drivers_ready = if self.driver.is_some() { 1 } else { 0 };

        BrainStatus {
            modalities,
            endpoints,
            drivers_ready,
        }
    }

    /// Resolve credentials for the brain (for flow credential injection).
    pub fn credentials_for(&self, _provider: &str) -> Option<carrier_types::brain::ProviderCredentials> {
        let mut env_vars = HashMap::new();
        if !self.config.api_key_env.is_empty() {
            if let Some(val) = carrier_types::env::get_secret(&self.config.api_key_env) {
                env_vars.insert(self.config.api_key_env.clone(), val);
            }
        }
        Some(carrier_types::brain::ProviderCredentials {
            provider_name: PROVIDER_NAME.to_string(),
            env_vars,
        })
    }

    // ── Convenience methods ─────────────────────────────────

    /// Get the model name for a modality (== the modality name itself).
    pub fn model_for(&self, modality: &str) -> String {
        if self.config.modalities.contains_key(modality) {
            modality.to_string()
        } else if self
            .config
            .modalities
            .contains_key(&self.config.default_modality)
        {
            self.config.default_modality.clone()
        } else {
            "unknown".to_string()
        }
    }

    /// Get the default modality name.
    pub fn default_modality(&self) -> &str {
        &self.config.default_modality
    }

    /// Check if a modality is available.
    pub fn has_modality(&self, modality: &str) -> bool {
        self.config.modalities.contains_key(modality)
    }

    /// Get the underlying config (for dashboard API).
    pub fn config(&self) -> &BrainConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
/// Implement the runtime Brain trait so agent_loop can use Brain methods.
#[async_trait]
impl BrainTrait for Brain {
    fn list_modalities(&self) -> Vec<ModalityInfo> {
        Brain::list_modalities(self)
    }

    fn endpoints_for(&self, modality: &str) -> Vec<ResolvedEndpoint> {
        Brain::endpoints_for(self, modality)
    }

    fn driver_for_endpoint(&self, endpoint_id: &str) -> Option<Arc<dyn LlmDriver>> {
        Brain::driver_for_endpoint(self, endpoint_id)
    }

    fn report(&self, report: EndpointReport) {
        Brain::report(self, report)
    }

    fn status(&self) -> BrainStatus {
        Brain::status(self)
    }

    fn credentials_for(&self, provider: &str) -> Option<carrier_types::brain::ProviderCredentials> {
        Brain::credentials_for(self, provider)
    }

    fn model_for(&self, modality: &str) -> String {
        Brain::model_for(self, modality)
    }

    fn has_modality(&self, modality: &str) -> bool {
        Brain::has_modality(self, modality)
    }
}

/// Brain initialization errors.
#[derive(Debug)]
pub enum BrainError {
    /// Driver creation failed.
    DriverCreation { error: String },
}

impl std::fmt::Display for BrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrainError::DriverCreation { error } => {
                write!(f, "Failed to create brain driver: {error}")
            }
        }
    }
}

impl std::error::Error for BrainError {}
