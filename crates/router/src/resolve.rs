// Route resolution (D13): the registry IS the filesystem. Two tiers —
// resolve_fast stats `aginx-<joined-prefix>` files directly (no metadata
// read, no dir listing); when that misses, build_table lists every
// executable aginx-* across the cmd dirs and reads their headers so
// aginx:name=/aginx:alias= routes resolve too. Dir order = precedence:
// AGINX_CMD_PATH defaults to /var/bin:/usr/bin so provisioned (writable)
// commands shadow baked ones, same later-wins precedent agsvc uses for
// its unit files.

use crate::meta::{self, Meta};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const DEFAULT_CMD_PATH: &str = "/var/bin:/usr/bin";

pub fn cmd_path_env() -> String {
    std::env::var("AGINX_CMD_PATH").unwrap_or_else(|_| DEFAULT_CMD_PATH.to_string())
}

pub fn cmd_dirs() -> Vec<PathBuf> {
    cmd_path_env()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn is_exec_file(p: &Path) -> bool {
    match fs::metadata(p) {
        Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// argv words → candidate routes, longest first, joined with '-'.
/// `aginx cam shot extra` → ["cam-shot-extra", "cam-shot", "cam"]. The k
/// returned is how many words the candidate consumed; the rest pass to
/// the target verbatim.
pub fn candidates(words: &[String]) -> Vec<(usize, String)> {
    (1..=words.len())
        .rev()
        .map(|k| (k, words[..k].join("-")))
        .collect()
}

/// Stat-only fast path: first (longest) joined prefix that names an
/// executable aginx-<candidate> file, dirs in AGINX_CMD_PATH order.
pub fn resolve_fast(words: &[String]) -> Option<(String, PathBuf, Vec<String>)> {
    let dirs = cmd_dirs();
    for (k, cand) in candidates(words) {
        for d in &dirs {
            let p = d.join(format!("aginx-{cand}"));
            if is_exec_file(&p) {
                return Some((cand, p, words[k..].to_vec()));
            }
        }
    }
    None
}

/// Every executable `aginx-*` file across the cmd dirs, in dir order. A
/// route seen in an earlier dir hides the same filename in later ones.
/// Dotfiles, the router itself and `.aginxmd` sidecars never qualify.
pub fn scan() -> Vec<(String, PathBuf)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for d in cmd_dirs() {
        let rd = match fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let name = match e.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if !name.starts_with("aginx-") || name.ends_with(".aginxmd") {
                continue;
            }
            let p = e.path();
            if !is_exec_file(&p) {
                continue;
            }
            let route = name["aginx-".len()..].to_string();
            if seen.insert(route.clone()) {
                out.push((route, p));
            }
        }
    }
    out
}

pub struct Entry {
    pub file_route: String,
    pub path: PathBuf,
    pub meta: Option<Meta>,
}

impl Entry {
    /// Effective route: aginx:name= override, else the filename.
    pub fn name(&self) -> &str {
        self.meta
            .as_ref()
            .and_then(|m| m.name.as_deref())
            .unwrap_or(&self.file_route)
    }
}

pub struct Table {
    pub entries: Vec<Entry>,
    /// effective route or alias → entry index; first claimant wins
    pub route_of: BTreeMap<String, usize>,
    /// "route 'x' claimed by A and B" — --check reports these
    pub collisions: Vec<String>,
}

/// Full registry with metadata: needed for menu/commands/--check and as the
/// fallback resolution tier (aginx:name= / aginx:alias= routes are not
/// visible to stat-only lookup).
pub fn build_table() -> Table {
    let mut entries: Vec<Entry> = Vec::new();
    for (route, p) in scan() {
        entries.push(Entry {
            file_route: route,
            meta: meta::read_for(&p),
            path: p,
        });
    }
    let mut route_of: BTreeMap<String, usize> = BTreeMap::new();
    let mut collisions: Vec<String> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let mut names = vec![e.name().to_string()];
        if let Some(m) = &e.meta {
            names.extend(m.aliases.iter().cloned());
        }
        for n in names {
            if n.is_empty() {
                continue;
            }
            match route_of.get(&n) {
                Some(&j) if j != i => collisions.push(format!(
                    "route '{n}' claimed by {} and {}",
                    entries[j].path.display(),
                    e.path.display()
                )),
                Some(_) => {}
                None => {
                    route_of.insert(n, i);
                }
            }
        }
    }
    Table {
        entries,
        route_of,
        collisions,
    }
}

/// Table-tier resolution for name/alias routes; same longest-prefix rule.
pub fn resolve_full<'a>(words: &[String], t: &'a Table) -> Option<(&'a Entry, Vec<String>)> {
    for (k, cand) in candidates(words) {
        if let Some(&i) = t.route_of.get(&cand) {
            return Some((&t.entries[i], words[k..].to_vec()));
        }
    }
    None
}

pub struct Suggestions {
    /// routes the token is a prefix of (excluding exact match)
    pub prefix: Vec<String>,
    /// routes within edit distance 2 (excluding prefix matches)
    pub typo: Vec<String>,
}

impl Suggestions {
    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty() && self.typo.is_empty()
    }
}

pub fn suggest(t: &Table, token: &str) -> Suggestions {
    let mut prefix = Vec::new();
    let mut typo = Vec::new();
    for r in t.route_of.keys() {
        if r == token {
            continue;
        }
        if r.starts_with(token) {
            prefix.push(r.clone());
        } else if edit_distance_within(token, r, 2) {
            typo.push(r.clone());
        }
    }
    Suggestions { prefix, typo }
}

/// Bounded Levenshtein: true when distance(a,b) <= max.
pub fn edit_distance_within(a: &str, b: &str, max: usize) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return false;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        let mut row_min = cur[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            row_min = row_min.min(cur[j + 1]);
        }
        if row_min > max {
            return false;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()] <= max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_longest_first() {
        let w: Vec<String> = ["cam", "shot", "extra"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            candidates(&w),
            vec![
                (3, "cam-shot-extra".to_string()),
                (2, "cam-shot".to_string()),
                (1, "cam".to_string()),
            ]
        );
    }

    #[test]
    fn edit_distance_bounds() {
        assert!(edit_distance_within("wet", "web", 2));
        assert!(edit_distance_within("scan", "scn", 2));
        assert!(!edit_distance_within("aaaa", "bbbb", 2));
        assert!(!edit_distance_within("voice-say", "cam-shot", 2));
    }
}
