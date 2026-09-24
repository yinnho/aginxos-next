//! Per-clone evolution strategy configuration.
//!
//! Reads `EVOLUTION.md` from the clone's workspace root. When missing,
//! uses conservative defaults.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Evolution mode — controls how aggressively the clone learns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionMode {
    /// Only extract clear factual knowledge (names, policies, procedures).
    #[default]
    Conservative,
    /// Extract facts + patterns + gaps. Good for general-purpose clones.
    Moderate,
    /// Extract everything including behavioral patterns. For training-heavy clones.
    Aggressive,
    /// Disable auto-evolution entirely. Manual knowledge management only.
    Disabled,
}

/// Per-clone evolution strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EvolutionConfig {
    /// How aggressively to extract knowledge.
    pub evolution_mode: EvolutionMode,
    /// Maximum number of knowledge files allowed.
    pub max_knowledge_files: usize,
    /// Maximum total knowledge storage in MB.
    pub knowledge_capacity_mb: usize,
    /// Whether to auto-compile knowledge (merge duplicates, compress).
    pub auto_compile: bool,
    /// Compile interval in hours.
    pub compile_interval_hours: u64,
    /// Days before unused knowledge is marked stale.
    pub bloat_stale_days: u32,
    /// Days before stale knowledge is deleted.
    pub bloat_delete_days: u32,
    /// Whether to send anonymized feedback to Hub.
    pub feedback_to_hub: bool,
    /// Whether autonomous self-growth (idle-time learn/create cron) is enabled
    /// for this clone. Requires the global master switch (`clone_lifecycle
    /// .self_growth_enabled`) to be on too. Default off.
    pub self_growth_enabled: bool,
    /// Self-growth cron interval in hours — how often the clone wakes to
    /// autonomously learn (and, if OA-bound, create article drafts). Default 6.
    pub self_growth_interval_hours: u64,
    /// V3: identity layer files that are frozen — evolution system may not modify.
    pub identity_frozen_files: Vec<String>,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            evolution_mode: EvolutionMode::Conservative,
            max_knowledge_files: 200,
            knowledge_capacity_mb: 50,
            auto_compile: true,
            compile_interval_hours: 24,
            bloat_stale_days: 30,
            bloat_delete_days: 60,
            feedback_to_hub: false,
            self_growth_enabled: false,
            self_growth_interval_hours: 6,
            identity_frozen_files: Vec::new(),
        }
    }
}

/// Read evolution config from workspace's EVOLUTION.md.
/// Returns default config if the file doesn't exist.
pub fn read_evolution_config(workspace: &Path) -> EvolutionConfig {
    let path = workspace.join("EVOLUTION.md");
    if !path.exists() {
        return EvolutionConfig::default();
    }

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return EvolutionConfig::default(),
    };

    parse_evolution_config(&content)
}

/// Parse EVOLUTION.md content — frontmatter as TOML/ YAML.
fn parse_evolution_config(content: &str) -> EvolutionConfig {
    let mut config = EvolutionConfig::default();

    let frontmatter = if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            &rest[..end]
        } else {
            return config;
        }
    } else {
        return config;
    };

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("evolution_mode:") {
            let v = val.trim().trim_matches('"').trim_matches('\'');
            config.evolution_mode = match v {
                "conservative" => EvolutionMode::Conservative,
                "moderate" => EvolutionMode::Moderate,
                "aggressive" => EvolutionMode::Aggressive,
                "disabled" => EvolutionMode::Disabled,
                _ => EvolutionMode::Conservative,
            };
        } else if let Some(val) = line.strip_prefix("max_knowledge_files:") {
            if let Ok(n) = val.trim().parse() {
                config.max_knowledge_files = n;
            }
        } else if let Some(val) = line.strip_prefix("knowledge_capacity_mb:") {
            if let Ok(n) = val.trim().parse() {
                config.knowledge_capacity_mb = n;
            }
        } else if let Some(val) = line.strip_prefix("auto_compile:") {
            config.auto_compile = val.trim() == "true";
        } else if let Some(val) = line.strip_prefix("bloat_stale_days:") {
            if let Ok(n) = val.trim().parse() {
                config.bloat_stale_days = n;
            }
        } else if let Some(val) = line.strip_prefix("bloat_delete_days:") {
            if let Ok(n) = val.trim().parse() {
                config.bloat_delete_days = n;
            }
        } else if let Some(val) = line.strip_prefix("feedback_to_hub:") {
            config.feedback_to_hub = val.trim() == "true";
        } else if let Some(val) = line.strip_prefix("self_growth_enabled:") {
            config.self_growth_enabled = val.trim() == "true";
        } else if let Some(val) = line.strip_prefix("self_growth_interval_hours:") {
            if let Ok(n) = val.trim().parse() {
                config.self_growth_interval_hours = n;
            }
        } else if let Some(val) = line.strip_prefix("identity_frozen:") {
            let v = val.trim().trim_matches('"').trim_matches('\'');
            config.identity_frozen_files = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    config
}

/// Check if evolution should proceed based on config and knowledge capacity.
pub fn should_evolve(config: &EvolutionConfig, knowledge_count: usize) -> bool {
    if config.evolution_mode == EvolutionMode::Disabled {
        return false;
    }
    if knowledge_count >= config.max_knowledge_files {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = EvolutionConfig::default();
        assert_eq!(config.evolution_mode, EvolutionMode::Conservative);
        assert_eq!(config.max_knowledge_files, 200);
        assert_eq!(config.knowledge_capacity_mb, 50);
        assert!(config.auto_compile);
        assert_eq!(config.bloat_stale_days, 30);
        assert_eq!(config.bloat_delete_days, 60);
        assert!(!config.feedback_to_hub);
        assert!(!config.self_growth_enabled);
        assert_eq!(config.self_growth_interval_hours, 6);
    }

    #[test]
    fn test_read_missing_file() {
        let tmp = TempDir::new().unwrap();
        let config = read_evolution_config(tmp.path());
        assert_eq!(config.evolution_mode, EvolutionMode::Conservative);
    }

    #[test]
    fn test_parse_full_config() {
        let content = r#"---
evolution_mode: "aggressive"
max_knowledge_files: 100
knowledge_capacity_mb: 25
auto_compile: false
bloat_stale_days: 14
bloat_delete_days: 30
feedback_to_hub: true
self_growth_enabled: true
self_growth_interval_hours: 12
---

## Custom rules
Some custom rules here.
"#;
        let config = parse_evolution_config(content);
        assert_eq!(config.evolution_mode, EvolutionMode::Aggressive);
        assert_eq!(config.max_knowledge_files, 100);
        assert_eq!(config.knowledge_capacity_mb, 25);
        assert!(!config.auto_compile);
        assert_eq!(config.bloat_stale_days, 14);
        assert_eq!(config.bloat_delete_days, 30);
        assert!(config.feedback_to_hub);
        // self_growth fields must actually parse (cf. compile_interval_hours,
        // which has no parse branch and silently stays at default).
        assert!(config.self_growth_enabled);
        assert_eq!(config.self_growth_interval_hours, 12);
    }

    #[test]
    fn test_self_growth_defaults_when_absent() {
        // Frontmatter without self_growth keys → defaults (off, 6h).
        let content = r#"---
evolution_mode: "conservative"
---
"#;
        let config = parse_evolution_config(content);
        assert!(!config.self_growth_enabled);
        assert_eq!(config.self_growth_interval_hours, 6);
    }

    #[test]
    fn test_should_evolve() {
        let config = EvolutionConfig::default();
        assert!(should_evolve(&config, 0));
        assert!(should_evolve(&config, 199));
        assert!(!should_evolve(&config, 200));

        let disabled = EvolutionConfig {
            evolution_mode: EvolutionMode::Disabled,
            ..Default::default()
        };
        assert!(!should_evolve(&disabled, 0));
    }

    #[test]
    fn test_read_from_file() {
        let tmp = TempDir::new().unwrap();
        let content = r#"---
evolution_mode: "moderate"
max_knowledge_files: 50
---
"#;
        fs::write(tmp.path().join("EVOLUTION.md"), content).unwrap();
        let config = read_evolution_config(tmp.path());
        assert_eq!(config.evolution_mode, EvolutionMode::Moderate);
        assert_eq!(config.max_knowledge_files, 50);
    }
}
