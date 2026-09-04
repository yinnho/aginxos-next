// aginx — AginxOS single-entry command router (D13: the bare command is
// the mother's face — typing a command is calling the mother by name).
//
// The command universe is a flat set of `aginx-*` executables; the
// filename IS the route and the filesystem IS the registry — dropping an
// executable into a cmd dir registers the command, no rebuild, no
// central list. Resolution is stat-only on the fast path; dispatch is
// execve, so exit codes and signals pass straight through (this process
// IS the target).
//
// Builtins: bare `aginx` / `aginx help` → menu; `aginx commands
// [--all|--json|--check]` → listing / D1 envelope / lint gate; `aginx
// agent send|list|status|create` → front-desk client over the server's
// UDS (the mother's conversation face, N1⑥).
// Intercepts (before any target code runs): `--help`/`-h` anywhere in
// argv prints route help and exits 0 — the target is never executed
// (the Omarchy `update aur --help` class of incident); a bare call to a
// command whose `aginx:args=` declares a required `<positional>` is
// refused with usage, exit 2; unknown commands exit 127 with
// did-you-mean.

mod agent;
mod meta;
mod resolve;

use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::exit;

use meta::Meta;
use resolve::{cmd_path_env, Entry};

fn main() {
    exit(run());
}

fn run() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        print_menu(false);
        return 0;
    }
    match argv[0].as_str() {
        "help" if argv.len() == 1 => {
            print_menu(false);
            0
        }
        "help" => cmd_help(&argv[1..]),
        "commands" => builtin_commands(&argv[1..]),
        "agent" => agent::run(&argv[1..]),
        _ if argv.iter().any(|a| is_help_flag(a)) => {
            let words: Vec<String> = argv.iter().filter(|a| !is_help_flag(a)).cloned().collect();
            if words.is_empty() {
                print_menu(false);
                0
            } else {
                cmd_help(&words)
            }
        }
        _ => dispatch(&argv),
    }
}

fn is_help_flag(a: &str) -> bool {
    a == "--help" || a == "-h"
}

/// Help for whatever the words resolve to; unknown words fall through the
/// normal 127 path so `aginx nope --help` still teaches.
fn cmd_help(words: &[String]) -> i32 {
    if let Some((route, path, _)) = resolve::resolve_fast(words) {
        print_help(&route, &path, meta::read_for(&path).as_ref());
        return 0;
    }
    let t = resolve::build_table();
    if let Some((e, _)) = resolve::resolve_full(words, &t) {
        let route = e.name().to_string();
        print_help(&route, &e.path, e.meta.as_ref());
        return 0;
    }
    unknown(words)
}

fn dispatch(words: &[String]) -> i32 {
    // Fast path: pure stat, no metadata read. The one exception is a bare
    // call (no trailing args) — that is the only dispatch shape whose
    // required-args guard needs the header, and bare calls are rare.
    if let Some((route, path, rest)) = resolve::resolve_fast(words) {
        if rest.is_empty() {
            if let Some(m) = meta::read_for(&path) {
                if m.requires_args() {
                    refuse_bare(&route, &path, &m);
                    return 2;
                }
            }
        }
        exec(&path, &rest)
    } else {
        // Table tier: aginx:name= / aginx:alias= routes.
        let t = resolve::build_table();
        if let Some((e, rest)) = resolve::resolve_full(words, &t) {
            if rest.is_empty() {
                if let Some(m) = &e.meta {
                    if m.requires_args() {
                        refuse_bare(e.name(), &e.path, m);
                        return 2;
                    }
                }
            }
            exec(&e.path, &rest)
        } else {
            unknown(words)
        }
    }
}

/// A command that declares required `<positional>` args was called bare.
fn refuse_bare(route: &str, path: &Path, m: &Meta) {
    eprintln!("aginx: 'aginx {route}' requires arguments");
    match &m.args {
        Some(a) => eprintln!("usage: aginx {route} {a}"),
        None => eprintln!("usage: aginx {route} …"),
    }
    eprintln!("path: {}", path.display());
    eprintln!("try: aginx {route} --help");
}

fn unknown(words: &[String]) -> i32 {
    let t = resolve::build_table();
    eprintln!("aginx: unknown command '{}'", words.join(" "));
    let mut s = resolve::suggest(&t, &words[0]);
    if s.is_empty() && words.len() > 1 {
        s = resolve::suggest(&t, words.last().unwrap());
    }
    if !s.prefix.is_empty() {
        let list = s
            .prefix
            .iter()
            .map(|r| format!("aginx {r}"))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("prefix matches: {list}");
    }
    if !s.typo.is_empty() {
        let list = s
            .typo
            .iter()
            .map(|r| format!("aginx {r}"))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("did you mean: {list}");
    }
    eprintln!("see: aginx commands --all");
    127
}

/// execve replaces this process; the child's exit status (code or signal)
/// becomes ours with nothing in between.
fn exec(path: &Path, rest: &[String]) -> ! {
    let cpath = match CString::new(path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("aginx: path contains NUL: {}", path.display());
            exit(126)
        }
    };
    let mut cargs = vec![cpath.clone()];
    for a in rest {
        match CString::new(a.as_str()) {
            Ok(c) => cargs.push(c),
            Err(_) => {
                eprintln!("aginx: argument contains NUL");
                exit(2)
            }
        }
    }
    let mut argvp: Vec<*const libc::c_char> = cargs.iter().map(|c| c.as_ptr()).collect();
    argvp.push(std::ptr::null());
    unsafe {
        libc::execv(cpath.as_ptr(), argvp.as_mut_ptr());
    }
    let err = std::io::Error::last_os_error();
    eprintln!("aginx: {}: {}", path.display(), err);
    exit(match err.raw_os_error() {
        Some(libc::ENOENT) => 127,
        _ => 126,
    })
}

fn print_menu(all: bool) {
    let groups = meta::load_groups();
    let t = resolve::build_table();
    println!("aginx — AginxOS command router (file-is-registry)");
    println!();
    println!("usage: aginx <command> [args…]");
    println!("       aginx <command> --help      command help (flag works anywhere)");
    println!("       aginx commands --all | --json | --check");
    println!("       aginx agent send [<名字>] <文本…>   跟母体/化身说话（前台）");
    println!();
    let mut by_group: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for e in &t.entries {
        let m = e.meta.as_ref();
        if m.map(|m| m.hidden).unwrap_or(false) && !all {
            continue;
        }
        let g = m
            .map(|m| m.group_or_derived(&e.file_route))
            .unwrap_or_else(|| e.file_route.split('-').next().unwrap_or("misc").to_string());
        by_group.entry(g).or_default().push(e);
    }
    if by_group.is_empty() {
        println!("no commands found (AGINX_CMD_PATH={})", cmd_path_env());
        return;
    }
    for (g, mut es) in by_group {
        match groups.get(&g) {
            Some(d) => println!("{g} — {d}"),
            None => println!("{g}"),
        }
        es.sort_by_key(|e| e.name().to_string());
        for e in es {
            let sum = e
                .meta
                .as_ref()
                .and_then(|m| m.summary.as_deref())
                .unwrap_or("");
            println!("  {:<24} {}", e.name(), sum);
        }
        println!();
    }
}

fn print_help(route: &str, path: &Path, m: Option<&Meta>) {
    let m = m.cloned().unwrap_or_default();
    println!("aginx {route} — {}", m.summary.as_deref().unwrap_or("(no aginx:summary=)"));
    println!();
    match &m.args {
        Some(a) => println!("usage: aginx {route} {a}"),
        None => println!("usage: aginx {route} [args…]"),
    }
    if !m.examples.is_empty() {
        println!();
        println!("examples:");
        for ex in &m.examples {
            println!("  aginx {route} {ex}");
        }
    }
    if !m.aliases.is_empty() {
        println!();
        println!("aliases: {}", m.aliases.join(", "));
    }
    println!();
    println!("path: {}", path.display());
}

fn builtin_commands(flags: &[String]) -> i32 {
    let mut all = false;
    let mut json = false;
    let mut check = false;
    for f in flags {
        match f.as_str() {
            "--all" => all = true,
            "--json" => json = true,
            "--check" => check = true,
            "--help" | "-h" => {
                println!("usage: aginx commands [--all|--json|--check]");
                println!("  --all    include hidden commands");
                println!("  --json   D1 envelope (ok/data/meta)");
                println!("  --check  lint the registry, exit 1 on errors");
                return 0;
            }
            other => {
                eprintln!("aginx commands: unknown flag '{other}'");
                eprintln!("usage: aginx commands [--all|--json|--check]");
                return 2;
            }
        }
    }
    if check {
        return lint();
    }
    if json {
        return print_json(all);
    }
    print_menu(all);
    0
}

/// D1 output contract: success envelope on stdout, one record per command.
fn print_json(all: bool) -> i32 {
    let t = resolve::build_table();
    let mut recs = Vec::new();
    for e in &t.entries {
        let m = e.meta.clone().unwrap_or_default();
        if m.hidden && !all {
            continue;
        }
        let name = e.name().to_string();
        let group = m.group_or_derived(&e.file_route);
        recs.push(serde_json::json!({
            "route": e.file_route,
            "name": name,
            "group": group,
            "summary": m.summary,
            "args": m.args,
            "examples": m.examples,
            "aliases": m.aliases,
            "hidden": m.hidden,
            "requires_sudo": m.requires_sudo,
            "path": e.path.display().to_string(),
        }));
    }
    let count = recs.len();
    let env = agio::ok_meta(
        serde_json::Value::Array(recs),
        serde_json::json!({
            "count": count,
            "cmd_path": cmd_path_env(),
            "groups_desc": meta::load_groups().len(),
        }),
    );
    agio::print(&env);
    0
}

/// Registry lint — the build gate. Errors (exit 1): route collisions,
/// missing summary, bad boolean, unknown key, compiled command without
/// .aginxmd sidecar, aginx:exec target missing. Warning (still exit 0):
/// group not registered in groups.desc.
fn lint() -> i32 {
    let mut errs = 0usize;
    let t = resolve::build_table();
    for c in &t.collisions {
        eprintln!("aginx check: collision: {c}");
        errs += 1;
    }
    let groups = meta::load_groups();
    let mut warned: Vec<String> = Vec::new();
    for e in &t.entries {
        let (m, ferrs) = meta::read_strict(&e.path);
        for msg in ferrs {
            eprintln!("aginx check: {}: {msg}", e.path.display());
            errs += 1;
        }
        if let Some(m) = m {
            let g = m.group_or_derived(&e.file_route);
            if !groups.contains_key(&g) && !warned.contains(&g) {
                eprintln!("aginx check: warning: group '{g}' not in groups.desc");
                warned.push(g);
            }
            if let Some(target) = &m.exec {
                if !exec_target_exists(target) {
                    eprintln!(
                        "aginx check: {}: aginx:exec target missing: {target}",
                        e.path.display()
                    );
                    errs += 1;
                }
            }
        }
    }
    if errs == 0 {
        println!("aginx check: {} commands OK", t.entries.len());
        0
    } else {
        eprintln!("aginx check: {errs} error(s)");
        1
    }
}

/// aginx:exec targets are either absolute paths or bare names resolved
/// against the cmd dirs plus the conventional system bin dirs.
fn exec_target_exists(target: &str) -> bool {
    if target.contains('/') {
        return Path::new(target).exists();
    }
    let mut dirs = resolve::cmd_dirs();
    for d in ["/bin", "/usr/bin", "/sbin", "/usr/sbin"] {
        dirs.push(d.into());
    }
    dirs.iter().any(|d| d.join(target).exists())
}
