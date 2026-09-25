// Route resolution (D13): the registry IS the filesystem. Three tiers,
// tier-major (FS.md: providers/ fully first, then tools/, then PATH) —
// a route claimed by an earlier tier shadows everything below it:
//
//   1. <home>/providers/<name>/<name>   (agent CLIs: codex, claude, …)
//   2. <home>/tools/<name>/<name>       (plain CLIs dropped into home)
//   3. <cmd dirs>/aginx-<name>          (baked/provisioned universe)
//
// <home> = AGINX_HOME > AGINX_CARRIER_HOME > /home; cmd dirs =
// AGINX_CMD_PATH (default /var/bin:/usr/bin, dir order = precedence).
// resolve_fast stats candidates directly (no metadata read, no dir
// listing); when that misses, build_table lists every executable across
// all tiers and reads their headers so aginx:name=/aginx:alias= routes
// resolve too. Within a tier, longest prefix wins.

use crate::meta::{self, Meta};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const DEFAULT_CMD_PATH: &str = "/var/bin:/usr/bin";

/// Home tiers in precedence order; `<home>/<tier>/<name>/<name>`.
pub const HOME_TIERS: [&str; 2] = ["providers", "tools"];

pub fn cmd_path_env() -> String {
    std::env::var("AGINX_CMD_PATH").unwrap_or_else(|_| DEFAULT_CMD_PATH.to_string())
}

/// Mother home for the providers/tools tiers. Same priority as
/// carrier_types::home_dir (AGINX_HOME > AGINX_CARRIER_HOME > /home),
/// reimplemented here so the router stays dependency-lean.
pub fn home_dir() -> PathBuf {
    for var in ["AGINX_HOME", "AGINX_CARRIER_HOME"] {
        if let Ok(h) = std::env::var(var) {
            if !h.is_empty() {
                return PathBuf::from(h);
            }
        }
    }
    PathBuf::from("/home")
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

/// Stat-only fast path over all three tiers, tier-major: providers
/// fully first, then tools, then the PATH dirs.
pub fn resolve_fast(words: &[String]) -> Option<(String, PathBuf, Vec<String>)> {
    resolve_fast_in(&home_dir(), &cmd_dirs(), words)
}

pub fn resolve_fast_in(
    home: &Path,
    dirs: &[PathBuf],
    words: &[String],
) -> Option<(String, PathBuf, Vec<String>)> {
    for tier in HOME_TIERS {
        let base = home.join(tier);
        for (k, cand) in candidates(words) {
            let p = base.join(&cand).join(&cand);
            if is_exec_file(&p) {
                return Some((cand, p, words[k..].to_vec()));
            }
        }
    }
    for (k, cand) in candidates(words) {
        for d in dirs {
            let p = d.join(format!("aginx-{cand}"));
            if is_exec_file(&p) {
                return Some((cand, p, words[k..].to_vec()));
            }
        }
    }
    None
}

/// Every executable across all tiers, in precedence order. A route seen
/// in an earlier tier hides the same route in later ones. Dotfiles, the
/// router itself and `.aginxmd` sidecars never qualify.
pub fn scan() -> Vec<(String, PathBuf)> {
    scan_in(&home_dir(), &cmd_dirs())
}

pub fn scan_in(home: &Path, dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    // Home tiers: <home>/<tier>/<name>/<name>, route = dir name.
    for tier in HOME_TIERS {
        let rd = match fs::read_dir(home.join(tier)) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let name = match e.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if name.starts_with('.') {
                continue;
            }
            let p = e.path().join(&name);
            if !is_exec_file(&p) {
                continue;
            }
            if seen.insert(name.clone()) {
                out.push((name, p));
            }
        }
    }
    // PATH tier: aginx-* files across the cmd dirs.
    for d in dirs {
        let rd = match fs::read_dir(d) {
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

    /// Fixture: providers/pi, tools/pi, PATH aginx-pi (same route in all
    /// three tiers); providers/cam, PATH aginx-cam-shot (tier-major
    /// cross-tier case); tools/git; PATH aginx-qr.
    fn three_tier_tree() -> (PathBuf, Vec<PathBuf>) {
        let home = testkit::tmp("router-home");
        let bins = testkit::tmp("router-bin");
        testkit::write_exec(&home.join("providers/pi/pi"), b"#!/bin/sh\n");
        testkit::write_exec(&home.join("tools/pi/pi"), b"#!/bin/sh\n");
        testkit::write_exec(&bins.join("aginx-pi"), b"#!/bin/sh\n");
        testkit::write_exec(&home.join("providers/cam/cam"), b"#!/bin/sh\n");
        testkit::write_exec(&bins.join("aginx-cam-shot"), b"#!/bin/sh\n");
        testkit::write_exec(&home.join("tools/git/git"), b"#!/bin/sh\n");
        testkit::write_exec(&bins.join("aginx-qr"), b"#!/bin/sh\n");
        (home, vec![bins])
    }

    fn words(s: &str) -> Vec<String> {
        s.split(' ').map(|w| w.to_string()).collect()
    }

    #[test]
    fn tier_major_shadowing() {
        let (home, dirs) = three_tier_tree();
        // Same route in all three tiers → providers wins.
        let (route, path, rest) = resolve_fast_in(&home, &dirs, &words("pi hi")).unwrap();
        assert_eq!(route, "pi");
        assert_eq!(path, home.join("providers/pi/pi"));
        assert_eq!(rest, vec!["hi".to_string()]);
        // tools-only route → tools.
        let (_, path, _) = resolve_fast_in(&home, &dirs, &words("git status")).unwrap();
        assert_eq!(path, home.join("tools/git/git"));
        // PATH-only route → aginx-<route>.
        let (route, path, rest) = resolve_fast_in(&home, &dirs, &words("qr")).unwrap();
        assert_eq!(route, "qr");
        assert_eq!(path, dirs[0].join("aginx-qr"));
        assert!(rest.is_empty());
        // Unknown → None.
        assert!(resolve_fast_in(&home, &dirs, &words("nope")).is_none());
    }

    #[test]
    fn tier_major_beats_longer_path_prefix() {
        let (home, dirs) = three_tier_tree();
        // providers/cam (1 word) shadows the longer PATH route
        // aginx-cam-shot (2 words) — tier-major, FS.md order.
        let (route, path, rest) = resolve_fast_in(&home, &dirs, &words("cam shot")).unwrap();
        assert_eq!(route, "cam");
        assert_eq!(path, home.join("providers/cam/cam"));
        assert_eq!(rest, vec!["shot".to_string()]);
    }

    #[test]
    fn longest_prefix_within_path_tier() {
        let home = testkit::tmp("router-empty-home");
        let bins = testkit::tmp("router-bin2");
        testkit::write_exec(&bins.join("aginx-cam"), b"#!/bin/sh\n");
        testkit::write_exec(&bins.join("aginx-cam-shot"), b"#!/bin/sh\n");
        let (route, _, rest) = resolve_fast_in(&home, &[bins], &words("cam shot x")).unwrap();
        assert_eq!(route, "cam-shot");
        assert_eq!(rest, vec!["x".to_string()]);
    }

    #[test]
    fn scan_collects_all_tiers_in_precedence_order() {
        let (home, dirs) = three_tier_tree();
        // Non-exec and hidden entries never qualify.
        std::fs::create_dir_all(home.join("providers/notexec")).unwrap();
        std::fs::write(home.join("providers/notexec/notexec"), b"#!/bin/sh\n").unwrap();
        std::fs::create_dir_all(home.join("tools/.hidden")).unwrap();
        testkit::write_exec(&home.join("tools/.hidden/.hidden"), b"#!/bin/sh\n");
        let got = scan_in(&home, &dirs);
        let routes: Vec<&str> = got.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(routes.len(), 5, "{routes:?}");
        let pos = |r: &str| routes.iter().position(|x| *x == r).unwrap();
        // Tier order: all providers before tools before PATH.
        assert!(pos("pi") < pos("git") && pos("cam") < pos("git"));
        assert!(pos("git") < pos("qr") && pos("git") < pos("cam-shot"));
        let by_route: BTreeMap<_, _> = got.iter().map(|(r, p)| (r.as_str(), p)).collect();
        assert_eq!(*by_route["pi"], home.join("providers/pi/pi"));
        assert_eq!(*by_route["cam"], home.join("providers/cam/cam"));
        assert_eq!(*by_route["git"], home.join("tools/git/git"));
        assert_eq!(*by_route["qr"], dirs[0].join("aginx-qr"));
        assert_eq!(*by_route["cam-shot"], dirs[0].join("aginx-cam-shot"));
    }

    #[test]
    fn home_dir_priority() {
        let _g = testkit::env_lock();
        let saved_home = std::env::var_os("AGINX_HOME");
        let saved_carrier = std::env::var_os("AGINX_CARRIER_HOME");
        std::env::set_var("AGINX_HOME", "/tmp/a");
        std::env::set_var("AGINX_CARRIER_HOME", "/tmp/b");
        assert_eq!(home_dir(), PathBuf::from("/tmp/a"));
        std::env::remove_var("AGINX_HOME");
        assert_eq!(home_dir(), PathBuf::from("/tmp/b"));
        std::env::remove_var("AGINX_CARRIER_HOME");
        assert_eq!(home_dir(), PathBuf::from("/home"));
        match saved_home {
            Some(v) => std::env::set_var("AGINX_HOME", v),
            None => std::env::remove_var("AGINX_HOME"),
        }
        match saved_carrier {
            Some(v) => std::env::set_var("AGINX_CARRIER_HOME", v),
            None => std::env::remove_var("AGINX_CARRIER_HOME"),
        }
    }
}
