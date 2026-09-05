//! aginx-pkg lib — package install engine (M26 agpkg, N4③b 改姓).
//!
//! v0 semantics preserved (docs/SYSTEM.md §6.1): a package is a static
//! musl binary at /var/bin/<name> with /var/bin/.<name>.prev for
//! rollback and /var/apps/<name>/ as its data dir; manifest lines are
//! `<name> <url> <sha256> [core|opt]` with absent 4th field = core;
//! `sync` self-heals core entries only; `opt-in` installs an opt entry
//! and seeds its launcher registry entry; installs are atomic
//! (.new → rename) and keep the previous binary on any failure.
//!
//! M26 adds the signed chain and the 四件套:
//!
//! * **Signature gate** — the default manifest `/etc/agpkg.manifest`
//!   requires a detached ed25519 sig at `.sig`, verified over the RAW
//!   bytes (aginx-sign lib — the SAME key/chain as aginx-update updates) before a
//!   single line is parsed or byte fetched. An explicit path argument
//!   (or AGPKG_MANIFEST env) is the adb dev loop: unsigned is allowed
//!   with a stderr note. Fail-closed everywhere else.
//! * **四件套 bundle** — when the artifact is a tar (ustar magic at
//!   offset 257), it must carry `bin/<name>` + `pkg.toml` + `SKILL.md`.
//!   Missing SKILL.md = install fails: a package without its skill doc
//!   does not install. SKILL.md lands in /var/lib/agpkg/skills/<name>/,
//!   a `[service]` table in pkg.toml is written verbatim as an agsvc
//!   overlay unit (/var/lib/agpkg/units/<name>.toml — the channel agsvc
//!   already scans) followed by `aginx-svc reload`. Extra files ride under
//!   skills/<name>/; a second entry under bin/ is rejected — one
//!   package, one binary.
//! * **Stamps** — /var/lib/agpkg/stamps/<name> records the sha256 that
//!   was installed, so `sync` sees tar-based packages as up-to-date
//!   (their manifest sha256 pins the tar, not the extracted binary).
//!   Pre-M26 bare-binary installs are healed by comparing the binary's
//!   own sha256, v0-style, then stamped.
//!
//! Paths are env-overridable (AGINX_PKG_BINDIR etc.) for host tests; on the
//! phone nobody sets them and the constants below are the truth.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use agio::ErrorType;

// ------------------------------------------------------------ paths

#[derive(Debug, Clone)]
pub struct Paths {
    pub bindir: PathBuf,
    pub appdir: PathBuf,
    pub skills: PathBuf,
    pub units: PathBuf,
    pub stamps: PathBuf,
    pub pkgfiles: PathBuf,
    pub dldir: PathBuf,
    pub manifest: PathBuf,
    pub downloader: PathBuf,
    pub svcctl: PathBuf,
    pub apps_d: PathBuf,
}

fn envp(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default))
}

impl Paths {
    pub fn from_env() -> Paths {
        Paths {
            bindir: envp("AGINX_PKG_BINDIR", "/var/bin"),
            appdir: envp("AGINX_PKG_APPDIR", "/var/apps"),
            skills: envp("AGINX_PKG_SKILLS", "/var/lib/agpkg/skills"),
            units: envp("AGINX_PKG_UNITS", "/var/lib/agpkg/units"),
            stamps: envp("AGINX_PKG_STAMPS", "/var/lib/agpkg/stamps"),
            pkgfiles: envp("AGINX_PKG_PKGFILES", "/var/lib/agpkg/pkgfiles"),
            dldir: envp("AGINX_PKG_DL", "/var/tmp/agpkg"),
            manifest: envp("AGINX_PKG_MANIFEST", "/etc/agpkg.manifest"),
            downloader: envp("AGINX_PKG_AGDL", "/usr/bin/aginx-download"),
            svcctl: envp("AGINX_PKG_AGCTL", "/usr/bin/aginx-svc"),
            apps_d: envp("AGINX_PKG_APPS_D", "/etc/apps.d"),
        }
    }
}

// ------------------------------------------------------------- errors

/// Install-shaped error: everything the CLI needs to print either the
/// human line or the D1 envelope, with the agio closed type set.
#[derive(Debug)]
pub struct Fail {
    pub etype: ErrorType,
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
}

impl Fail {
    pub fn new(etype: ErrorType, code: &str, message: impl Into<String>) -> Fail {
        Fail { etype, code: code.to_string(), message: message.into(), hint: None }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Fail {
        self.hint = Some(hint.into());
        self
    }
    pub fn envelope(&self) -> serde_json::Value {
        match &self.hint {
            Some(h) => agio::fail_hint(self.etype, &self.code, &self.message, h),
            None => agio::fail(self.etype, &self.code, &self.message),
        }
    }
}

fn io_fail(code: &str, message: impl std::fmt::Display) -> Fail {
    Fail::new(ErrorType::Io, code, message.to_string())
}

fn usage_fail(message: impl Into<String>) -> Fail {
    Fail::new(ErrorType::Usage, "usage", message)
}

// ---------------------------------------------------------- manifest

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier {
    Core,
    Opt,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub tier: Tier,
}

/// Parse manifest text: `<name> <url> <sha256> [core|opt]`, '#'
/// comments and blank lines skipped. A line with a name but no
/// url/sha256 is a hard parse error (v0 warned per-line at sync; the
/// Rust gate refuses the whole file so a typo can never silently
/// drop a package from self-heal).
pub fn parse_manifest(src: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 {
            return Err(format!("line {}: want '<name> <url> <sha256> [core|opt]'", i + 1));
        }
        let tier = if f.get(3) == Some(&"opt") { Tier::Opt } else { Tier::Core };
        out.push(Entry {
            name: f[0].to_string(),
            url: f[1].to_string(),
            sha256: f[2].to_string(),
            tier,
        });
    }
    Ok(out)
}

/// Load + (unless dev) signature-gate a manifest file. `dev` is true
/// for an explicit path argument / AGINX_PKG_MANIFEST env — the adb dev
/// loop; the default /etc/agpkg.manifest always requires the detached
/// sig. `pubkey_b64` is passed in so tests can sign with their own key;
/// production passes aginx_sign::AGINX_PUBKEY_B64.
pub fn load_manifest(path: &Path, dev: bool, pubkey_b64: &str) -> Result<Vec<Entry>, Fail> {
    let body = std::fs::read(path)
        .map_err(|e| io_fail("manifest_read", format!("read {}: {e}", path.display())))?;
    if dev {
        eprintln!("aginx-pkg: {path_display} unsigned (explicit path — dev override)", path_display = path.display());
    } else {
        let sig = {
            let mut s = path.as_os_str().to_os_string();
            s.push(".sig");
            std::fs::read_to_string(PathBuf::from(s))
                .map_err(|e| {
                    Fail::new(
                        ErrorType::Auth,
                        "manifest_unsigned",
                        format!("{}: no detached signature ({e}) — refusing", path.display()),
                    )
                    .with_hint(format!(
                        "sign it on the host: aginx-sign sign .local/keys/aginx.key {}",
                        path.display()
                    ))
                })?
        };
        aginx_sign::verify_with_key(pubkey_b64, &body, &sig).map_err(|e| {
            Fail::new(
                ErrorType::Auth,
                "manifest_bad_sig",
                format!("{}: signature INVALID ({e}) — refusing", path.display()),
            )
            .with_hint("the manifest changed after signing, or was not signed by the update key")
        })?;
    }
    let text = String::from_utf8_lossy(&body);
    parse_manifest(&text).map_err(|e| io_fail("manifest_parse", format!("{path}: {e}", path = path.display())))
}

// -------------------------------------------------------------- hash

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = [0u8; 64 << 10];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ------------------------------------------------------------ install

#[derive(Debug, PartialEq)]
pub enum Kind {
    /// bare static binary (v0 path)
    Binary,
    /// 四件套 tar
    Bundle { skill: bool, unit: bool },
}

/// Max bytes we will buffer for one bundle member (or the whole bare
/// binary when it rides a tar). Phone packages are single-digit MiB;
/// anything near this is a mistake, not a package.
const MEMBER_MAX: u64 = 256 << 20;

/// Sniff the artifact: ustar magic ("ustar" at offset 257) = bundle.
fn is_tar(path: &Path) -> std::io::Result<bool> {
    use std::io::Seek;
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 5];
    let got = f.seek(std::io::SeekFrom::Start(257))
        .and_then(|_| f.read(&mut magic))
        .unwrap_or(0);
    Ok(got == 5 && &magic == b"ustar")
}

/// A gzipped tar has no ustar magic at 257 and would silently fall through
/// `install_file` to the v0 flat-binary path — raw gzip copied to /var/bin
/// (M32c receipt: "installed" fine, `agf` then died with `magic 1F8B`).
/// Sniff it up front so the failure is loud at install time, not boot time.
fn is_gzip(path: &Path) -> std::io::Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 2];
    let got = f.read(&mut magic).unwrap_or(0);
    Ok(got == 2 && &magic == b"\x1f\x8b")
}

/// Install a local artifact (already downloaded or adb-pushed) whose
/// content must hash to `sha256`. Writes the stamp on success so
/// `sync` can see this exact version as current.
pub fn install_file(p: &Paths, name: &str, src: &Path, sha256: &str) -> Result<Kind, Fail> {
    let meta = std::fs::metadata(src)
        .map_err(|_| io_fail("no_src", format!("no such file: {}", src.display())))?;
    if !meta.is_file() {
        return Err(io_fail("no_src", format!("{src} is not a regular file", src = src.display())));
    }
    let got = sha256_file(src).map_err(|e| io_fail("hash", format!("{}: {e}", src.display())))?;
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(io_fail(
            "sha_mismatch",
            format!("sha256 mismatch for {name}: got {got} want {sha256}"),
        )
        .with_hint("the file is not what the caller pinned — re-download or fix the pin"));
    }
    let kind = if is_gzip(src).map_err(|e| io_fail("sniff", format!("{}: {e}", src.display())))? {
        return Err(io_fail(
            "pkg_gzip",
            format!("{name}: gzipped artifact refused — aginx-pkg bundles are uncompressed ustar tar (or a flat binary)"),
        )
        .with_hint("repack without -z: tar --format=ustar -cf out.tar bin/<name> pkg.toml SKILL.md"));
    } else if is_tar(src).map_err(|e| io_fail("sniff", format!("{}: {e}", src.display())))? {
        install_bundle(p, name, src)?
    } else {
        place_binary(p, name, &std::fs::read(src).map_err(|e| io_fail("read", format!("{}: {e}", src.display())))?)?;
        Kind::Binary
    };
    mkdir_all(&p.appdir.join(name))?;
    write_stamp(p, name, sha256)?;
    Ok(kind)
}

/// Normalize a tar member path ("./bin/x", "bin//x" → "bin/x") and
/// reject absolute paths / ".." walk-outs before anything is buffered.
fn member_path(raw: &str) -> Result<String, Fail> {
    let parts: Vec<&str> = raw.split('/').filter(|s| !s.is_empty()).collect();
    if parts.contains(&"..") || raw.starts_with('/') {
        return Err(io_fail("pkg_unsafe_path", format!("bundle member escapes: {raw}"))
            .with_hint("packages must be relative tar trees"));
    }
    Ok(parts.join("/"))
}

fn read_member<R: Read>(e: &mut tar::Entry<R>) -> Result<Vec<u8>, Fail> {
    if e.header().entry_size().unwrap_or(0) > MEMBER_MAX {
        return Err(io_fail("pkg_member_huge", "bundle member exceeds 256 MiB — not a package"));
    }
    let mut buf = Vec::new();
    e.read_to_end(&mut buf).map_err(|e| io_fail("pkg_read", format!("tar read: {e}")))?;
    if buf.len() as u64 > MEMBER_MAX {
        return Err(io_fail("pkg_member_huge", "bundle member exceeds 256 MiB — not a package"));
    }
    Ok(buf)
}

fn install_bundle(p: &Paths, name: &str, src: &Path) -> Result<Kind, Fail> {
    let f = std::fs::File::open(src).map_err(|e| io_fail("open", format!("{}: {e}", src.display())))?;
    let mut ar = tar::Archive::new(f);
    // Two install surfaces in one tar:
    //   files/**  — a payload TREE, streamed straight to a temp dir
    //               (a CPython tree is ~150 MB; buffering it in the
    //               members map would double memory for nothing). Only
    //               valid with pkg.toml `exec` — see below.
    //   everything else — buffered small members: bin/<name>, pkg.toml,
    //               SKILL.md, extra files that ride with the skill.
    // Dirs are not kept (parents are created by their children);
    // symlinks under files/ are recreated, elsewhere ignored.
    let mut members: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut tree_streamed = false;
    let tmp_tree = p.pkgfiles.join(".tmp-install");
    let _ = std::fs::remove_dir_all(&tmp_tree);
    let stream_dir = tmp_tree.clone();
    let mut tree_symlinks: Vec<(String, String)> = Vec::new();
    let entries = ar.entries().map_err(|e| io_fail("pkg_read", format!("tar walk: {e}")))?;
    for e in entries {
        let mut e = e.map_err(|e| io_fail("pkg_read", format!("tar member: {e}")))?;
        let raw = e.path().map_err(|e| io_fail("pkg_read", format!("tar path: {e}")))?.to_string_lossy().into_owned();
        let key = member_path(&raw)?;
        let et = e.header().entry_type();
        if et.is_dir() {
            continue;
        }
        if key.starts_with("files/") || key == "files" {
            if !et.is_file() && !et.is_symlink() && !et.is_hard_link() {
                continue; // fifo/device in a package tree: not ours
            }
            if et.is_symlink() || et.is_hard_link() {
                let target = e.link_name()
                    .map_err(|e| io_fail("pkg_read", format!("tar link: {e}")))?
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if target.starts_with('/') {
                    return Err(io_fail("pkg_unsafe_link", format!("files/{raw} -> {target}: absolute link"))
                        .with_hint("links inside a package tree must stay relative"));
                }
                tree_streamed = true;
                tree_symlinks.push((key, target));
                continue;
            }
            let rel = key.strip_prefix("files/").unwrap_or_default();
            if rel.is_empty() {
                continue;
            }
            tree_streamed = true;
            stream_member(&mut e, &stream_dir, rel)?;
            continue;
        }
        if !et.is_file() {
            continue;
        }
        members.insert(key, read_member(&mut e)?);
    }

    let pkg_raw = members.remove("pkg.toml").ok_or_else(|| {
        io_fail("pkg_missing_manifest", "bundle missing pkg.toml")
            .with_hint(format!("四件套 = bin/{name} + pkg.toml + SKILL.md"))
    })?;
    let skill = members.remove("SKILL.md").ok_or_else(|| {
        // THE 四件套 rule: no skill doc, no install.
        io_fail("skill_missing", "bundle missing SKILL.md — a package without its skill doc does not install")
            .with_hint("add SKILL.md at the tar root describing what this package is for and how to use it")
    })?;
    let bin_key = format!("bin/{name}");
    let bin = members.remove(&bin_key);
    if bin.is_none() && !tree_streamed {
        // v0 error precedence: a missing face outranks pkg.toml
        // problems — and with no files/ tree, no later `exec` could
        // rescue the bundle.
        let _ = std::fs::remove_dir_all(&tmp_tree);
        return Err(io_fail("pkg_missing_bin", format!("bundle missing {bin_key}"))
            .with_hint(format!("四件套 = bin/{name} + pkg.toml + SKILL.md")));
    }

    // pkg.toml: name must match; `exec` (optional) names the entry under
    // files/ that becomes the /var/bin face as a SYMLINK into the tree —
    // for runtimes whose executable resolves its own libs/stdlib
    // relative to its real path (CPython); [service] (optional) becomes
    // an agsvc unit.
    let doc: toml::Value = toml::from_str(&String::from_utf8_lossy(&pkg_raw))
        .map_err(|e| io_fail("pkg_manifest_parse", format!("pkg.toml: {e}")))?;
    let tbl = doc.as_table().ok_or_else(|| io_fail("pkg_manifest_parse", "pkg.toml: want a table at top level"))?;
    let pkg_name = tbl.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
        io_fail("pkg_manifest_parse", "pkg.toml: 'name' is required").with_hint(format!("name = \"{name}\""))
    })?;
    if pkg_name != name {
        return Err(io_fail(
            "pkg_name_mismatch",
            format!("pkg.toml name '{pkg_name}' != install name '{name}'"),
        )
        .with_hint("install name and pkg.toml name must be the same string"));
    }
    let exec = tbl.get("exec").and_then(|v| v.as_str());

    // The face: either a flat binary member (bin/<name>) or a symlink
    // into the files/ tree (pkg.toml exec). Exactly one.
    match (exec, bin.is_some()) {
        (Some(ex), true) => {
            let _ = std::fs::remove_dir_all(&tmp_tree);
            return Err(io_fail("pkg_face_twice", format!("bundle has both exec = \"{ex}\" and {bin_key}"))
                .with_hint("a package face is either a flat bin/<name> member or pkg.toml exec — not both"));
        }
        (Some(ex), false) => {
            if member_path(ex).is_err() || ex.starts_with('/') {
                let _ = std::fs::remove_dir_all(&tmp_tree);
                return Err(io_fail("pkg_exec", format!("pkg.toml exec '{ex}' is not a safe relative path")));
            }
            // exists = streamed file OR a pending tree symlink (links are
            // materialized at commit, so the exec face may legitimately
            // still be only in tree_symlinks here).
            let pending_link = tree_symlinks.iter().any(|(k, _)| k == &format!("files/{ex}"));
            if !pending_link && !tmp_tree.join(ex).exists() {
                let _ = std::fs::remove_dir_all(&tmp_tree);
                return Err(io_fail("pkg_exec", format!("pkg.toml exec 'files/{ex}' not in the bundle"))
                    .with_hint("exec names a member under files/ — ship it in the tar"));
            }
        }
        (None, false) => {
            let _ = std::fs::remove_dir_all(&tmp_tree);
            return Err(io_fail("pkg_missing_bin", format!("bundle missing {bin_key}"))
                .with_hint(format!("四件套 = bin/{name} + pkg.toml + SKILL.md")));
        }
        (None, true) => {}
    }
    // one package, one binary — a second bin/ member is a packaging bug
    // that would silently not install, so it is a hard error.
    if let Some(extra) = members.keys().find(|k| k.starts_with("bin/")) {
        let _ = std::fs::remove_dir_all(&tmp_tree);
        return Err(io_fail("pkg_extra_bin", format!("unexpected {extra} — one binary per package"))
            .with_hint(format!("the package binary is {bin_key}; anything else belongs beside SKILL.md")));
    }

    let mut has_unit = false;
    if let Some(svc) = tbl.get("service") {
        let svc = svc.as_table().ok_or_else(|| io_fail("pkg_service", "pkg.toml [service] must be a table"))?;
        let cmd = svc.get("cmd").and_then(|v| v.as_str()).ok_or_else(|| {
            io_fail("pkg_service", "pkg.toml [service] requires 'cmd'").with_hint(format!("cmd = \"/var/bin/{name}\""))
        })?;
        if !cmd.starts_with('/') {
            return Err(io_fail("pkg_service", format!("[service] cmd '{cmd}' is not an absolute path")));
        }
        // agsvc's unit parser only knows str/bool/str-array values.
        for (k, v) in svc {
            let ok = v.is_str() || v.is_bool()
                || v.as_array().map(|a| a.iter().all(|i| i.is_str())).unwrap_or(false);
            if !ok {
                return Err(io_fail(
                    "pkg_service",
                    format!("[service] {k}: only string, bool, and string-array values are valid in a unit"),
                ));
            }
        }
        write_unit(p, name, svc)?;
        has_unit = true;
    }

    // Commit the tree (if any): files/ is already streamed under the
    // temp dir with its modes; swap it in wholesale. No .prev — a tree
    // package rolls back by re-syncing (its bytes are pinned by the
    // manifest sha, and sync is self-healing every boot).
    let tree_used = exec.is_some();
    if tree_used {
        for (key, target) in &tree_symlinks {
            let rel = key.strip_prefix("files/").unwrap_or(key);
            let dst = tmp_tree.join(rel);
            std::fs::create_dir_all(dst.parent().unwrap_or(&tmp_tree))
                .map_err(|e| io_fail("pkg_tree", format!("{}: {e}", dst.display())))?;
            std::os::unix::fs::symlink(target, &dst)
                .map_err(|e| io_fail("pkg_tree", format!("link {key} -> {target}: {e}")))?;
        }
        let final_tree = p.pkgfiles.join(name);
        mkdir_all(&p.pkgfiles)?;
        let _ = std::fs::remove_dir_all(&final_tree);
        std::fs::rename(&tmp_tree, &final_tree)
            .map_err(|e| io_fail("pkg_tree", format!("{}: {e}", final_tree.display())))?;
    }

    // The face + SKILL.md (and any extra files that ride with the skill).
    match exec {
        Some(ex) => place_symlink_face(p, name, &p.pkgfiles.join(name).join(ex))?,
        None => place_binary(p, name, bin.as_ref().unwrap())?,
    }
    let skill_dir = p.skills.join(name);
    mkdir_all(&skill_dir)?;
    write_644(&skill_dir.join("SKILL.md"), &skill)
        .map_err(|e| io_fail("skill_write", format!("{}: {e}", skill_dir.join("SKILL.md").display())))?;
    for (rel, bytes) in &members {
        let dst = skill_dir.join(rel);
        write_644(&dst, bytes).map_err(|e| io_fail("skill_write", format!("{}: {e}", dst.display())))?;
    }
    if has_unit {
        reload_units(p);
    }
    Ok(Kind::Bundle { skill: true, unit: has_unit })
}

/// Stream one files/ member to its place in the temp tree, keeping the
/// tar's mode (a tree package's executables must arrive executable).
fn stream_member<R: Read>(e: &mut tar::Entry<R>, root: &Path, rel: &str) -> Result<(), Fail> {
    use std::os::unix::fs::PermissionsExt;
    let dst = root.join(rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_fail("pkg_tree", format!("{}: {e}", parent.display())))?;
    }
    let mut out = std::fs::File::create(&dst).map_err(|e| io_fail("pkg_tree", format!("{}: {e}", dst.display())))?;
    let mut take = std::io::BufReader::new(std::io::Read::by_ref(e)).take(MEMBER_MAX + 1);
    std::io::copy(&mut take, &mut out)
        .map_err(|e| io_fail("pkg_tree", format!("{}: {e}", dst.display())))?;
    if take.limit() == 0 {
        return Err(io_fail("pkg_member_huge", format!("files/{rel} exceeds 256 MiB — not a package member")));
    }
    let mode = e.header().mode().unwrap_or(0o644) & 0o777;
    std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode))
        .map_err(|e| io_fail("pkg_tree", format!("{}: {e}", dst.display())))?;
    Ok(())
}

/// Make /var/bin/<name> a symlink pointing at the tree entry (relative,
/// so the tree and the face move together). Replaces whatever face was
/// there — file, symlink, or nothing.
fn place_symlink_face(p: &Paths, name: &str, target: &Path) -> Result<(), Fail> {
    mkdir_all(&p.bindir)?;
    let face = p.bindir.join(name);
    let rel = relative_path(&face, target).ok_or_else(|| {
        io_fail("pkg_face", format!("cannot relate {name} face to {}", target.display()))
    })?;
    let _ = std::fs::remove_file(&face);
    std::os::unix::fs::symlink(&rel, &face)
        .map_err(|e| io_fail("pkg_face", format!("{name} -> {}: {e}", rel.display())))?;
    Ok(())
}

/// Relative path for a symlink at `link` pointing at `to` (both
/// absolute, pure string arithmetic). Resolution anchors at the link's
/// DIRECTORY — the link's own name is not part of the walk.
fn relative_path(link: &Path, to: &Path) -> Option<PathBuf> {
    let dir = link.parent()?;
    let f: Vec<_> = dir.components().collect();
    let t: Vec<_> = to.components().collect();
    let mut i = 0;
    while i < f.len() && i < t.len() && f[i] == t[i] {
        i += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in i..f.len() {
        parts.push("..".into());
    }
    for c in &t[i..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        return None;
    }
    Some(PathBuf::from(parts.join("/")))
}

/// Write the agsvc overlay unit: [unit] name + the package's [service]
/// table verbatim (aginx-svcd scans /var/lib/agpkg/units — M16's overlay
/// channel, finally written by its intended author).
fn write_unit(p: &Paths, name: &str, svc: &toml::map::Map<String, toml::Value>) -> Result<(), Fail> {
    let mut doc = toml::map::Map::new();
    let mut unit = toml::map::Map::new();
    unit.insert("name".to_string(), toml::Value::String(name.to_string()));
    doc.insert("unit".to_string(), toml::Value::Table(unit));
    doc.insert("service".to_string(), toml::Value::Table(svc.clone()));
    let text = toml::to_string(&toml::Value::Table(doc))
        .map_err(|e| io_fail("unit_write", format!("serialize unit: {e}")))?;
    mkdir_all(&p.units)?;
    write_644(&p.units.join(format!("{name}.toml")), text.as_bytes())
        .map_err(|e| io_fail("unit_write", format!("{}: {e}", p.units.join(format!("{name}.toml")).display())))?;
    Ok(())
}

/// Best-effort `aginx-svc reload`: the unit file is on disk either way and
/// agsvc rescans on restart — a failed reload is a warning, never a
/// failed install.
fn reload_units(p: &Paths) {
    match Command::new(&p.svcctl).arg("reload").status() {
        Ok(s) if s.success() => {}
        _ => eprintln!(
            "aginx-pkg: warning: {} reload failed — unit picked up on next aginx-svcd restart",
            p.svcctl.display()
        ),
    }
}

/// Atomic binary place with .prev retention (v0 semantics): same-fs
/// .new file, chmod 755, rename over the target; previous binary kept
/// as .<name>.prev for `rollback`.
fn place_binary(p: &Paths, name: &str, bytes: &[u8]) -> Result<(), Fail> {
    use std::os::unix::fs::PermissionsExt;
    mkdir_all(&p.bindir)?;
    let cur = p.bindir.join(name);
    if cur.exists() {
        std::fs::copy(&cur, p.bindir.join(format!(".{name}.prev")))
            .map_err(|e| io_fail("prev", format!("backup {name}: {e}")))?;
    }
    let new = p.bindir.join(format!(".{name}.new"));
    std::fs::write(&new, bytes).map_err(|e| io_fail("install", format!("{}: {e}", new.display())))?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| io_fail("chmod", format!("{}: {e}", new.display())))?;
    std::fs::rename(&new, &cur).map_err(|e| io_fail("install", format!("{}: {e}", cur.display())))?;
    Ok(())
}

/// Write a file with mode 0644 (fs::write inherits 0666&~umask and the
/// phone runs umask 0 — skills/units are agent-consumed docs, they
/// should not be world-writable).
fn write_644(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
}

fn mkdir_all(p: &Path) -> Result<(), Fail> {
    std::fs::create_dir_all(p).map_err(|e| io_fail("mkdir", format!("{}: {e}", p.display())))
}

fn write_stamp(p: &Paths, name: &str, sha256: &str) -> Result<(), Fail> {
    mkdir_all(&p.stamps)?;
    write_644(&p.stamps.join(name), format!("{sha256}\n").as_bytes())
        .map_err(|e| io_fail("stamp", format!("{}: {e}", p.stamps.join(name).display())))
}

fn read_stamp(p: &Paths, name: &str) -> Option<String> {
    std::fs::read_to_string(p.stamps.join(name)).ok().map(|s| s.trim().to_string())
}

// ------------------------------------------------------------- fetch

/// Download via aginx-download (the phone's only TLS fetcher) with the gh-proxy
/// retry for github.com URLs (TLS resets on some networks; content is
/// sha256-pinned either way), then install. The previous binary is
/// kept on any failure (nothing is touched until install_file's own
/// atomic rename).
pub fn fetch_install(p: &Paths, entry: &Entry) -> Result<Kind, Fail> {
    mkdir_all(&p.dldir)?;
    let dl = p.dldir.join(&entry.name);
    let _ = std::fs::remove_file(&dl);
    let mut ok = Command::new(&p.downloader).arg(&entry.url).arg(&dl).status().map(|s| s.success()).unwrap_or(false);
    if !ok && entry.url.starts_with("https://github.com/") {
        eprintln!("aginx-pkg: retrying via gh-proxy.com");
        ok = Command::new(&p.downloader)
            .arg(format!("https://gh-proxy.com/{}", entry.url))
            .arg(&dl)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    if !ok || !dl.exists() {
        let _ = std::fs::remove_file(&dl);
        return Err(io_fail("download_failed", format!("{}: aginx-download could not fetch {}", entry.name, entry.url))
            .with_hint("check network; github.com URLs retry through gh-proxy.com automatically"));
    }
    let r = install_file(p, &entry.name, &dl, &entry.sha256);
    let _ = std::fs::remove_file(&dl);
    r
}

// ---------------------------------------------------------- seed_app

/// Seed the launcher registry entry (§12.3) so an installed package
/// shows up on the launcher without an OS source change. Known apps
/// keep their curated /etc/apps.d seed; the default assumes unknown
/// UIs are PC-designed TUIs (codex/grok class, scale = 3).
pub fn seed_app(p: &Paths, name: &str) -> Result<(), Fail> {
    let dir = p.appdir.join(name);
    mkdir_all(&dir)?;
    let curated = p.apps_d.join(format!("{name}.toml"));
    if curated.exists() {
        std::fs::copy(&curated, dir.join("app.toml"))
            .map_err(|e| io_fail("seed_app", format!("{}: {e}", curated.display())))?;
        return Ok(());
    }
    std::fs::write(
        dir.join("app.toml"),
        format!("name = \"{name}\"\nbinary = \"{}\"\nargs = \"\"\nscale = 3\n", p.bindir.join(name).display()),
    )
    .map_err(|e| io_fail("seed_app", format!("{}: {e}", dir.join("app.toml").display())))?;
    Ok(())
}

// -------------------------------------------------------- subcommands

/// One row of output for the query commands. `lines` is the human face
/// (printed in order); `data`/`meta` carry the --json envelope payload.
#[derive(Debug, Default)]
pub struct CmdOut {
    pub lines: Vec<String>,
    pub data: Vec<serde_json::Value>,
    pub meta: serde_json::Value,
}

impl CmdOut {
    fn count(self) -> CmdOut {
        let n = self.data.len();
        let mut m = self.meta.as_object().cloned().unwrap_or_default();
        m.insert("count".to_string(), serde_json::json!(n));
        CmdOut { lines: self.lines, data: self.data, meta: serde_json::Value::Object(m) }
    }
}

pub fn cmd_sync(p: &Paths, manifest: Option<&Path>, pubkey_b64: &str) -> Result<i32, Fail> {
    let path = manifest.unwrap_or(&p.manifest);
    if !path.exists() {
        println!("aginx-pkg: no manifest {} — nothing to sync", path.display());
        return Ok(0);
    }
    let entries = load_manifest(path, manifest.is_some() || std::env::var_os("AGPKG_MANIFEST").is_some(), pubkey_b64)?;
    let mut rc = 0;
    for e in &entries {
        if e.tier == Tier::Opt {
            continue;
        }
        // up-to-date = stamp matches AND the binary is actually there.
        // Stamp alone is not enough: a rootfs swap (or any rm) wipes
        // /var/bin while the stamps ride the state tar — trusting the
        // stamp alone left the phone "up to date" with nothing installed
        // (observed 2026-09-03, first swap with /var/lib in the state
        // tar). Else the v0 legacy check on the binary's own hash, which
        // also heals the stamp for pre-M26 installs.
        let bin = p.bindir.join(&e.name);
        if read_stamp(p, &e.name).as_deref() == Some(e.sha256.as_str()) && bin.exists() {
            println!("aginx-pkg: {} up to date", e.name);
            continue;
        }
        let cur = p.bindir.join(&e.name).exists().then(|| sha256_file(&p.bindir.join(&e.name)).ok()).flatten();
        if cur.as_deref() == Some(e.sha256.as_str()) {
            write_stamp(p, &e.name, &e.sha256)?;
            println!("aginx-pkg: {} up to date", e.name);
            continue;
        }
        match cur {
            Some(_) => println!("aginx-pkg: {} stale — downloading", e.name),
            None => println!("aginx-pkg: {} missing — downloading", e.name),
        }
        match fetch_install(p, e) {
            Ok(_) => println!("aginx-pkg: installed {} ({})", e.name, e.sha256),
            Err(f) => {
                eprintln!("aginx-pkg: {} sync FAILED (kept previous): {}", e.name, f.message);
                rc = 1;
            }
        }
    }
    Ok(rc)
}

pub fn cmd_available(p: &Paths, manifest: Option<&Path>, pubkey_b64: &str) -> Result<CmdOut, Fail> {
    let mut out = CmdOut::default();
    let path = manifest.unwrap_or(&p.manifest);
    if !path.exists() {
        return Ok(out);
    }
    let entries = load_manifest(path, manifest.is_some() || std::env::var_os("AGPKG_MANIFEST").is_some(), pubkey_b64)?;
    for e in entries.iter().filter(|e| e.tier == Tier::Opt) {
        if p.bindir.join(&e.name).exists() {
            continue;
        }
        out.lines.push(e.name.clone());
        out.data.push(serde_json::json!({"name": e.name, "url": e.url, "sha256": e.sha256}));
    }
    Ok(out.count())
}

pub fn cmd_opt_in(p: &Paths, name: &str, pubkey_b64: &str) -> Result<(), Fail> {
    if !p.manifest.exists() {
        return Err(io_fail("no_manifest", format!("no manifest {}", p.manifest.display())));
    }
    let entries = load_manifest(&p.manifest, std::env::var_os("AGPKG_MANIFEST").is_some(), pubkey_b64)?;
    let e = entries.iter().find(|e| e.name == name).ok_or_else(|| {
        Fail::new(ErrorType::NotFound, "not_in_manifest", format!("{name} not in manifest"))
    })?;
    if e.tier != Tier::Opt {
        return Err(usage_fail(format!("{name} is not an opt entry (sync handles core)")));
    }
    let cur = sha256_file(&p.bindir.join(name)).ok();
    if cur.as_deref() == Some(e.sha256.as_str()) {
        write_stamp(p, name, &e.sha256)?;
        seed_app(p, name)?;
        println!("aginx-pkg: {name} already installed — seeded launcher entry");
        return Ok(());
    }
    fetch_install(p, e)?;
    seed_app(p, name)?;
    println!("aginx-pkg: opted in {name}");
    Ok(())
}

pub fn cmd_rollback(p: &Paths, name: &str) -> Result<(), Fail> {
    let prev = p.bindir.join(format!(".{name}.prev"));
    if !prev.exists() {
        return Err(Fail::new(ErrorType::NotFound, "no_prev", format!("no previous version of {name}")));
    }
    std::fs::rename(&prev, p.bindir.join(name))
        .map_err(|e| io_fail("rollback", format!("{}: {e}", prev.display())))?;
    // The stamp describes the version we just reverted away from —
    // drop it so the next sync sees the package as stale and re-heals
    // to the manifest version (v0 had no stamps; this keeps that
    // observable behavior).
    let _ = std::fs::remove_file(p.stamps.join(name));
    println!("aginx-pkg: rolled back {name}");
    Ok(())
}

pub fn cmd_list(p: &Paths) -> Result<CmdOut, Fail> {
    let mut out = CmdOut::default();
    let rd = std::fs::read_dir(&p.bindir)
        .map_err(|e| io_fail("list", format!("{}: {e}", p.bindir.display())))?;
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    for n in &names {
        let sha = sha256_file(&p.bindir.join(n)).unwrap_or_default();
        let skill = p.skills.join(n).join("SKILL.md").exists();
        let unit = p.units.join(format!("{n}.toml")).exists();
        let mut flags = String::new();
        if skill {
            flags.push_str("skill,");
        }
        if unit {
            flags.push_str("unit");
        }
        if flags.ends_with(',') {
            flags.pop();
        }
        out.lines.push(format!("{n:<16} {:<10} {}", if flags.is_empty() { "-" } else { &flags }, &sha[..12.min(sha.len())]));
        out.data.push(serde_json::json!({
            "name": n,
            "sha256": sha,
            "stamp": read_stamp(p, n),
            "skill": skill,
            "unit": unit,
        }));
    }
    Ok(out.count())
}

pub fn usage() -> &'static str {
    "usage: aginx-pkg install <name> <src> <sha256> | sync [manifest] | available [manifest] [--json] \
     | opt-in <name> | rollback <name> | list [--json]"
}

// -------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    use rand_core::{OsRng, RngCore};
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        testkit::tmp(&format!("aginx-pkg-{tag}"))
    }

    fn paths(root: &Path) -> Paths {
        Paths {
            bindir: root.join("bin"),
            appdir: root.join("apps"),
            skills: root.join("skills"),
            units: root.join("units"),
            stamps: root.join("stamps"),
            pkgfiles: root.join("pkgfiles"),
            dldir: root.join("dl"),
            manifest: root.join("manifest"),
            downloader: root.join("downloader"),
            svcctl: root.join("svcctl"),
            apps_d: root.join("apps.d"),
        }
    }

    fn build_tar(path: &Path, members: &[(&str, &[u8])]) {
        let f = fs::File::create(path).unwrap();
        let mut b = tar::Builder::new(f);
        for (name, data) in members {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.finish().unwrap();
    }

    const PKG_TOML: &str = "name = \"dup\"\nversion = \"0.1.0\"\ndesc = \"test\"\n";
    const SERVICE_TOML: &str = "name = \"dup\"\n[service]\ncmd = \"/var/bin/dup\"\nargs = [\"serve\"]\n";

    /// Hand-rolled member the Builder refuses to create (path escape):
    /// a valid ustar header for `../evil`, 1 data byte, zero padding.
    fn append_escape_member(archive: &Path) {
        let mut f = fs::OpenOptions::new().append(true).open(archive).unwrap();
        use std::io::Write;
        let mut h = [0u8; 512];
        h[..8].copy_from_slice(b"../evil\0");
        h[100..107].copy_from_slice(b"0000001"); // size, octal
        h[108..115].copy_from_slice(b"0000000"); // mtime
        h[156] = b'0'; // regular file
        h[257..263].copy_from_slice(b"ustar\0"); h[263..265].copy_from_slice(b"00");
        // checksum field (148..156) counts as eight spaces; the sum below
        // adds 8 * 32 for it directly
        let sum: u32 = h.iter().map(|b| *b as u32).sum::<u32>() + 8 * 32;
        let chk = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(chk.as_bytes());
        f.write_all(&h).unwrap();
        let mut data = [0u8; 512];
        data[0] = b'x';
        f.write_all(&data).unwrap();
    }

    fn keypair() -> (SigningKey, String) {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let sk = SigningKey::from_bytes(&seed);
        let pub_b64 = B64.encode(sk.verifying_key().as_ref());
        (sk, pub_b64)
    }

    fn write_signed(root: &Path, name: &str, body: &str, sk: &SigningKey) -> PathBuf {
        let p = root.join(name);
        fs::write(&p, body).unwrap();
        fs::write(root.join(format!("{name}.sig")), B64.encode(sk.sign(body.as_bytes()).to_bytes())).unwrap();
        p
    }

    #[test]
    fn manifest_parse_tiers_comments_and_errors() {
        let src = "# head\n\naginx url deadbeef core\ngrok url beefdead opt\nplain url cafebabe\n";
        let es = parse_manifest(src).unwrap();
        assert_eq!(es.len(), 3);
        assert_eq!(es[0].tier, Tier::Core);
        assert_eq!(es[1].tier, Tier::Opt);
        // absent 4th field = core
        assert_eq!(es[2].tier, Tier::Core);
        assert!(parse_manifest("broken\n").is_err());
    }

    #[test]
    fn unsigned_default_manifest_is_refused() {
        let root = tmp("unsigned");
        fs::write(root.join("manifest"), "aginx url deadbeef\n").unwrap();
        // no .sig sibling -> fail closed, typed auth
        let f = load_manifest(&root.join("manifest"), false, "irrelevant").unwrap_err();
        assert_eq!(f.code, "manifest_unsigned");
        assert_eq!(f.envelope()["error"]["type"], "auth");
        // dev override allows it
        assert!(load_manifest(&root.join("manifest"), true, "irrelevant").is_ok());
    }

    #[test]
    fn manifest_sig_verifies_with_real_key_and_rejects_tamper() {
        let root = tmp("sig");
        let (sk, pub_b64) = keypair();
        let p = write_signed(&root, "m", "aginx url deadbeef\n", &sk);
        assert!(load_manifest(&p, false, &pub_b64).is_ok());
        // tamper: same sig, different bytes
        fs::write(&p, "aginx url deadbeee\n").unwrap();
        let f = load_manifest(&p, false, &pub_b64).unwrap_err();
        assert_eq!(f.code, "manifest_bad_sig");
    }

    #[test]
    fn install_binary_path_matches_v0() {
        let root = tmp("bin0");
        let p = paths(&root);
        let src = root.join("tool.bin");
        fs::write(&src, b"#!/bin/sh\necho v0\n").unwrap();
        let sha = sha256_hex(b"#!/bin/sh\necho v0\n");
        assert_eq!(install_file(&p, "tool", &src, &sha).unwrap(), Kind::Binary);
        // placed, exec bit, stamp, app dir
        assert_eq!(fs::read_to_string(p.bindir.join("tool")).unwrap(), "#!/bin/sh\necho v0\n");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(p.bindir.join("tool")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(fs::read_to_string(p.stamps.join("tool")).unwrap().trim(), sha);
        assert!(p.appdir.join("tool").is_dir());
        // sha mismatch refused, previous kept
        fs::write(&src, b"tampered\n").unwrap();
        let f = install_file(&p, "tool", &src, &sha).unwrap_err();
        assert_eq!(f.code, "sha_mismatch");
        assert!(f.hint.is_some());
        assert_eq!(fs::read_to_string(p.bindir.join("tool")).unwrap(), "#!/bin/sh\necho v0\n");
        // second install keeps .prev
        fs::write(&src, b"v1\n").unwrap();
        let sha1 = sha256_hex(b"v1\n");
        install_file(&p, "tool", &src, &sha1).unwrap();
        assert_eq!(fs::read_to_string(p.bindir.join(".tool.prev")).unwrap(), "#!/bin/sh\necho v0\n");
    }

    #[test]
    fn bundle_without_skill_does_not_install() {
        let root = tmp("noskill");
        let p = paths(&root);
        let t = root.join("p.tar");
        build_tar(&t, &[("bin/dup", b"BIN"), ("pkg.toml", PKG_TOML.as_bytes())]);
        let sha = sha256_file(&t).unwrap();
        let f = install_file(&p, "dup", &t, &sha).unwrap_err();
        assert_eq!(f.code, "skill_missing");
        assert!(f.message.contains("SKILL.md"));
        // nothing landed
        assert!(!p.bindir.join("dup").exists());
        assert!(!p.skills.join("dup").exists());
    }

    #[test]
    fn gzipped_tar_refused_loudly() {
        // M32c receipt: a `tar -czf` bundle "installed" fine and died at
        // exec with `magic 1F8B` — the gzip header hides ustar magic at
        // 257, so it fell through to the v0 flat-binary path. It must be
        // refused at install time instead.
        let root = tmp("gzrefuse");
        let p = paths(&root);
        let inner = root.join("inner.tar");
        build_tar(
            &inner,
            &[("bin/agf", b"BIN"), ("pkg.toml", PKG_TOML.as_bytes()), ("SKILL.md", b"# s\n")],
        );
        let gz = root.join("p.tar.gz");
        // the sniff only reads the 2 magic bytes; a gzip header glued onto
        // the tar body is enough to model a `tar -czf` artifact
        let mut gzbytes = b"\x1f\x8b".to_vec();
        gzbytes.extend_from_slice(&fs::read(&inner).unwrap());
        fs::write(&gz, &gzbytes).unwrap();
        let sha = sha256_file(&gz).unwrap();
        let f = install_file(&p, "agf", &gz, &sha).unwrap_err();
        assert_eq!(f.code, "pkg_gzip");
        assert!(f.hint.as_deref().unwrap().contains("ustar"));
        // nothing landed anywhere
        assert!(!p.bindir.join("agf").exists());
        assert!(!p.skills.join("agf").exists());
    }

    #[test]
    fn bundle_happy_path_lands_all_four() {
        let root = tmp("happy");
        let p = paths(&root);
        fs::create_dir_all(&p.apps_d).unwrap();
        fs::write(p.apps_d.join("dup.toml"), "name = DUP\ncurated\n").unwrap();
        let t = root.join("p.tar");
        build_tar(
            &t,
            &[
                ("./bin/dup", b"DUPBIN"),
                ("pkg.toml", SERVICE_TOML.as_bytes()),
                ("SKILL.md", b"# dup skill\n"),
                ("assets/extra.txt", b"extra"),
            ],
        );
        let sha = sha256_file(&t).unwrap();
        assert_eq!(
            install_file(&p, "dup", &t, &sha).unwrap(),
            Kind::Bundle { skill: true, unit: true }
        );
        assert_eq!(fs::read(p.bindir.join("dup")).unwrap(), b"DUPBIN");
        assert_eq!(fs::read_to_string(p.skills.join("dup").join("SKILL.md")).unwrap(), "# dup skill\n");
        assert_eq!(fs::read_to_string(p.skills.join("dup").join("assets/extra.txt")).unwrap(), "extra");
        // unit = [unit] name + verbatim [service]
        let unit = fs::read_to_string(p.units.join("dup.toml")).unwrap();
        assert!(unit.contains("name = \"dup\""));
        assert!(unit.contains("cmd = \"/var/bin/dup\""));
        assert_eq!(fs::read_to_string(p.stamps.join("dup")).unwrap().trim(), sha);
    }

    #[test]
    fn bundle_rejects_name_mismatch_extra_bin_and_traversal() {
        let root = tmp("reject");
        let p = paths(&root);
        // pkg.toml name != install name (bin/<install-name> IS present)
        let t = root.join("a.tar");
        build_tar(&t, &[("bin/other", b"B"), ("pkg.toml", PKG_TOML.as_bytes()), ("SKILL.md", b"s")]);
        let f = install_file(&p, "other", &t, &sha256_file(&t).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_name_mismatch");
        // install name with no matching bin member
        let f = install_file(&p, "ghost", &t, &sha256_file(&t).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_missing_bin");
        // a second binary
        build_tar(
            &t,
            &[("bin/dup", b"B"), ("bin/spare", b"S"), ("pkg.toml", PKG_TOML.as_bytes()), ("SKILL.md", b"s")],
        );
        let f = install_file(&p, "dup", &t, &sha256_file(&t).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_extra_bin");
        // path traversal
        let t2 = root.join("b.tar");
        build_tar(&t2, &[("bin/dup", b"B"), ("pkg.toml", PKG_TOML.as_bytes()), ("SKILL.md", b"s")]);
        // the Builder writes a 2-block EOF marker; inject the escape
        // member BEFORE it (strip, append, re-add) or the reader stops
        // early and never sees the malicious member.
        {
            use std::io::Write;
            let end = fs::metadata(&t2).unwrap().len() - 1024;
            let f = fs::OpenOptions::new().read(true).write(true).open(&t2).unwrap();
            f.set_len(end).unwrap();
            drop(f);
            append_escape_member(&t2);
            let mut f = fs::OpenOptions::new().append(true).open(&t2).unwrap();
            f.write_all(&[0u8; 1024]).unwrap();
        }
        // the tar crate itself refuses `..` members at path() time; our
        // member_path guard is the second layer. Either way: refused,
        // nothing landed.
        let f = install_file(&p, "dup", &t2, &sha256_file(&t2).unwrap()).unwrap_err();
        assert!(f.code == "pkg_unsafe_path" || f.code == "pkg_read", "got {}", f.code);
        assert!(!p.bindir.join("dup").exists());
        // missing pkg.toml
        let t3 = root.join("c.tar");
        build_tar(&t3, &[("bin/dup", b"B"), ("SKILL.md", b"s")]);
        let f = install_file(&p, "dup", &t3, &sha256_file(&t3).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_missing_manifest");
        // missing bin/<name>
        let t4 = root.join("d.tar");
        build_tar(&t4, &[("bin/wrong", b"B"), ("pkg.toml", PKG_TOML.as_bytes()), ("SKILL.md", b"s")]);
        let f = install_file(&p, "dup", &t4, &sha256_file(&t4).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_missing_bin");
    }

    /// Tar with a files/ tree: control members at the root, streamed
    /// files with explicit modes, and symlink members inside the tree.
    fn build_tree_tar(
        path: &Path,
        control: &[(&str, &[u8])],
        tree_files: &[(&str, &[u8], u32)],
        tree_links: &[(&str, &str)],
    ) {
        let f = fs::File::create(path).unwrap();
        let mut b = tar::Builder::new(f);
        for (name, data) in control {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        for (name, data, mode) in tree_files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(*mode);
            h.set_cksum();
            b.append_data(&mut h, format!("files/{name}"), *data).unwrap();
        }
        for (name, target) in tree_links {
            let mut h = tar::Header::new_gnu();
            h.set_size(0);
            h.set_mode(0o777);
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_cksum();
            b.append_link(&mut h, format!("files/{name}"), *target).unwrap();
        }
        b.finish().unwrap();
    }

    const TREE_TOML: &str = "name = \"python3\"\nversion = \"3.12\"\nexec = \"bin/python3\"\n";

    #[test]
    fn tree_bundle_faces_symlink_and_keeps_modes() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("tree");
        let p = paths(&root);
        let t = root.join("p.tar");
        build_tree_tar(
            &t,
            &[("pkg.toml", TREE_TOML.as_bytes()), ("SKILL.md", b"# python3 skill\n")],
            &[
                ("bin/python3", b"ELF", 0o755),
                ("lib/python312/os.py", b"import sys\n", 0o644),
            ],
            &[("bin/python3.12", "python3")],
        );
        let sha = sha256_file(&t).unwrap();
        assert_eq!(
            install_file(&p, "python3", &t, &sha).unwrap(),
            Kind::Bundle { skill: true, unit: false }
        );
        // face is a symlink that resolves INTO the tree, relative
        let face = p.bindir.join("python3");
        assert!(face.is_symlink());
        assert_eq!(fs::read(&face).unwrap(), b"ELF");
        let lnk = fs::read_link(&face).unwrap().to_string_lossy().into_owned();
        assert!(!lnk.starts_with('/'), "face must be relative: {lnk}");
        // tree landed with modes and its internal symlink
        let elf = p.pkgfiles.join("python3/bin/python3");
        assert_eq!(fs::read(&elf).unwrap(), b"ELF");
        assert_eq!(fs::metadata(&elf).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(
            fs::metadata(p.pkgfiles.join("python3/lib/python312/os.py")).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::read_link(p.pkgfiles.join("python3/bin/python3.12")).unwrap().to_string_lossy(),
            "python3"
        );
        // skill + stamp as usual
        assert!(p.skills.join("python3/SKILL.md").is_file());
        assert_eq!(fs::read_to_string(p.stamps.join("python3")).unwrap().trim(), sha);
        // a wiped tree leaves a dangling face -> exists() false -> sync
        // takes the download path (the M26 gate, tree edition)
        fs::remove_dir_all(p.pkgfiles.join("python3")).unwrap();
        assert!(!p.bindir.join("python3").exists());
    }

    #[test]
    fn tree_bundle_face_rules() {
        let root = tmp("treeface");
        let p = paths(&root);
        // exec AND a flat bin member: two faces
        let t = root.join("a.tar");
        build_tree_tar(
            &t,
            &[("bin/python3", b"X"), ("pkg.toml", TREE_TOML.as_bytes()), ("SKILL.md", b"s")],
            &[("bin/python3", b"ELF", 0o755)],
            &[],
        );
        let f = install_file(&p, "python3", &t, &sha256_file(&t).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_face_twice");
        // exec pointing at a member that is not in the tar
        let t2 = root.join("b.tar");
        build_tree_tar(
            &t2,
            &[("pkg.toml", TREE_TOML.as_bytes()), ("SKILL.md", b"s")],
            &[("lib/x.py", b"print(1)\n", 0o644)],
            &[],
        );
        let f = install_file(&p, "python3", &t2, &sha256_file(&t2).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_exec");
        assert!(!p.pkgfiles.join("python3").exists());
        assert!(!p.pkgfiles.join(".tmp-install").exists(), "failed installs leave no temp tree");
        // absolute link inside the tree
        let t3 = root.join("c.tar");
        build_tree_tar(
            &t3,
            &[("pkg.toml", TREE_TOML.as_bytes()), ("SKILL.md", b"s")],
            &[("bin/python3", b"ELF", 0o755)],
            &[("lib/evil", "/etc/passwd")],
        );
        let f = install_file(&p, "python3", &t3, &sha256_file(&t3).unwrap()).unwrap_err();
        assert_eq!(f.code, "pkg_unsafe_link");
    }

    #[test]
    fn sync_stamp_legacy_heal_and_failure_paths() {
        let root = tmp("sync");
        let p = paths(&root);
        let (sk, pub_b64) = keypair();

        // legacy pre-M26 install: binary on disk matches manifest sha,
        // no stamp -> healed + stamped, no download attempted (downloader is
        // a path that does not exist).
        let bin = b"legacy-bin";
        let sha = sha256_hex(bin);
        fs::create_dir_all(&p.bindir).unwrap();
        fs::write(p.bindir.join("old"), bin).unwrap();
        let m = write_signed(&root, "m", &format!("old http://x {sha}\nnew http://y {sha} opt\n"), &sk);
        assert_eq!(cmd_sync(&p, Some(&m), &pub_b64).unwrap(), 0);
        assert_eq!(fs::read_to_string(p.stamps.join("old")).unwrap().trim(), sha);

        // stamp match -> up to date even though bindir binary differs
        fs::write(p.bindir.join("old"), b"tar-extracted-different").unwrap();
        assert_eq!(cmd_sync(&p, Some(&m), &pub_b64).unwrap(), 0);

        // stamp present but binary GONE — a rootfs swap wipes /var/bin
        // while the stamps ride the state tar; up-to-date must require
        // the binary too, so this takes the download path (downloader is a
        // nonexistent path here -> rc 1, stamp kept for the next round).
        fs::remove_file(p.bindir.join("old")).unwrap();
        assert_eq!(cmd_sync(&p, Some(&m), &pub_b64).unwrap(), 1);
        assert_eq!(fs::read_to_string(p.stamps.join("old")).unwrap().trim(), sha);

        // default manifest absent -> quiet no-op (v0), not an error
        assert_eq!(cmd_sync(&p, None, &pub_b64).unwrap(), 0);
        // explicit path that does not exist -> same quiet no-op (v0)
        assert_eq!(cmd_sync(&p, Some(&root.join("nonexistent")), &pub_b64).unwrap(), 0);
    }

    #[test]
    fn sync_downloads_via_stub_downloader() {
        let root = tmp("fetch");
        let p = paths(&root);
        // stub downloader: copies a fixture to $2
        let payload = root.join("payload.bin");
        fs::write(&payload, b"NEWBIN").unwrap();
        let stub = root.join("downloader");
        fs::write(&stub, format!("#!/bin/sh\ncp '{}' \"$2\"\n", payload.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        let sha = sha256_hex(b"NEWBIN");
        let m = root.join("m");
        fs::write(&m, format!("new http://x/{sha} {sha}\n")).unwrap();
        // explicit path = dev override, unsigned ok
        assert_eq!(cmd_sync(&p, Some(&m), "irrelevant").unwrap(), 0);
        assert_eq!(fs::read_to_string(p.bindir.join("new")).unwrap(), "NEWBIN");
        assert_eq!(fs::read_to_string(p.stamps.join("new")).unwrap().trim(), sha);
    }

    #[test]
    fn opt_in_tier_and_seed_rules() {
        let root = tmp("optin");
        let (sk, pub_b64) = keypair();
        let mut p = paths(&root);
        let bin = b"OPTBIN";
        let sha = sha256_hex(bin);
        p.manifest = write_signed(&root, "m", &format!("corepkg http://x {sha}\noptpkg http://x {sha} opt\n"), &sk);
        // not in manifest
        let f = cmd_opt_in(&p, "ghost", &pub_b64).unwrap_err();
        assert_eq!(f.code, "not_in_manifest");
        // core entry refuses opt-in
        let f = cmd_opt_in(&p, "corepkg", &pub_b64).unwrap_err();
        assert_eq!(f.code, "usage");
        // already installed + matching sha -> seeds launcher entry
        fs::create_dir_all(&p.bindir).unwrap();
        fs::write(p.bindir.join("optpkg"), bin).unwrap();
        cmd_opt_in(&p, "optpkg", &pub_b64).unwrap();
        let app = fs::read_to_string(p.appdir.join("optpkg").join("app.toml")).unwrap();
        assert!(app.contains("scale = 3"));
        assert!(app.contains("/bin/optpkg"));
    }

    #[test]
    fn rollback_restores_prev_and_drops_stamp() {
        let root = tmp("rollback");
        let p = paths(&root);
        fs::create_dir_all(&p.bindir).unwrap();
        fs::write(p.bindir.join("tool"), b"v1").unwrap();
        fs::write(p.bindir.join(".tool.prev"), b"v0").unwrap();
        fs::create_dir_all(&p.stamps).unwrap();
        fs::write(p.stamps.join("tool"), "v1sha\n").unwrap();
        cmd_rollback(&p, "tool").unwrap();
        assert_eq!(fs::read_to_string(p.bindir.join("tool")).unwrap(), "v0");
        assert!(!p.stamps.join("tool").exists());
        let f = cmd_rollback(&p, "tool").unwrap_err();
        assert_eq!(f.code, "no_prev");
    }

    #[test]
    fn list_records_shape() {
        let root = tmp("list");
        let p = paths(&root);
        fs::create_dir_all(&p.bindir).unwrap();
        fs::write(p.bindir.join(".dotfile"), b"x").unwrap(); // hidden, skipped
        fs::write(p.bindir.join("tool"), b"BIN").unwrap();
        fs::create_dir_all(p.skills.join("tool")).unwrap();
        fs::write(p.skills.join("tool").join("SKILL.md"), b"s").unwrap();
        let out = cmd_list(&p).unwrap();
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.meta["count"], 1);
        assert_eq!(out.data[0]["name"], "tool");
        assert_eq!(out.data[0]["skill"], true);
        assert_eq!(out.data[0]["unit"], false);
        assert_eq!(out.data[0]["stamp"], serde_json::Value::Null);
    }
}
