//! Topological partition of flow steps into execution layers.

use carrier_types::flow::StepDef;

/// Partition flow steps into topological execution layers. Returns an error on
/// cycles or references to unknown step ids.
pub(crate) fn partition_flow_steps(steps: &[StepDef]) -> Result<Vec<Vec<&StepDef>>, String> {
    use std::collections::{HashMap, HashSet};

    let map: HashMap<&str, &StepDef> = steps.iter().map(|s| (s.id.as_str(), s)).collect();

    // Validate dependencies exist.
    for s in steps {
        for d in &s.depends_on {
            if !map.contains_key(d.as_str()) {
                return Err(format!("step '{}' depends on unknown step '{}'", s.id, d));
            }
        }
    }

    // Cycle detection (DFS).
    let mut visited: HashSet<String> = HashSet::new();
    for s in steps {
        if has_cycle(&s.id, &map, &mut HashSet::new(), &mut visited) {
            return Err(format!(
                "dependency cycle detected involving step '{}'",
                s.id
            ));
        }
    }

    // Layer assignment: layer = max(dep.layer) + 1, or 0 if no deps.
    let mut layer_of: HashMap<String, usize> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for s in steps {
            let computed = if s.depends_on.is_empty() {
                0
            } else {
                s.depends_on
                    .iter()
                    .filter_map(|d| layer_of.get(d))
                    .copied()
                    .max()
                    .map(|l| l + 1)
                    .unwrap_or(0)
            };
            let cur = layer_of.entry(s.id.clone()).or_insert(0);
            if computed > *cur {
                *cur = computed;
                changed = true;
            }
        }
    }

    let max_layer = layer_of.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<&StepDef>> = vec![Vec::new(); max_layer + 1];
    for s in steps {
        let l = layer_of[&s.id];
        layers[l].push(s);
    }
    Ok(layers)
}

fn has_cycle(
    id: &str,
    map: &std::collections::HashMap<&str, &StepDef>,
    on_stack: &mut std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
) -> bool {
    use std::collections::HashSet;
    if on_stack.contains(id) {
        return true;
    }
    if visited.contains(id) {
        return false;
    }
    visited.insert(id.to_string());
    on_stack.insert(id.to_string());
    if let Some(s) = map.get(id) {
        for d in &s.depends_on {
            if has_cycle(d, map, on_stack, visited) {
                return true;
            }
        }
    }
    on_stack.remove(id);
    let _ = HashSet::<String>::new();
    false
}
