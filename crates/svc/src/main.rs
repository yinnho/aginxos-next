// aginx-svcd — the AginxOS service supervisor (M16, docs/SYSTEM.md §12.1;
// N4③b 搬入新仓改姓，原 agsvc).
//
// busybox init stays PID 1 and respawns this process
// (`::respawn:/usr/libexec/aginx/aginx-svcd`); aginx-svcd owns every other daemon. Children
// carry PR_SET_PDEATHSIG(SIGKILL), so an aginx-svcd crash takes them down too —
// init respawns aginx-svcd, which respawns everything else, with no
// double-spawn window.
//
// Readiness contract (Redox-init shape): `type = notify` spawns pass a
// pipe write end as fd 3 + env AGINX_SVC_NOTIFY=3; one byte written = ready,
// EOF = died before ready. Foreign binaries (gateway/carrier/browser)
// use `type = simple` (alive + grace = ready) until they adopt the
// /run/aginx-svc/<name>.sock contract (§12.2).
//
// Restart policy: exponential backoff 100 ms -> 10 s; circuit breaker
// parks the unit in `failed` after 5 exits within 60 s until an explicit
// `aginx-svc start`. Missing binaries are `absent`, not failed — provisioning
// may land them later, so they re-check every 30 s.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aginx_svc::{
    kmsg, load_units, log_path, path_exists, socket_for, SvcType, Unit, CTL_SOCK, LOG_DIR,
    UNIT_DIRS, ABSENT_RECHECK_MS, BACKOFF_MAX_MS, BACKOFF_START_MS, BREAKER_N,
    BREAKER_WINDOW_S, DEP_WAIT_MS, SIMPLE_GRACE_MS, STABLE_S,
};

/// A notify-type unit that stays `starting` this long without a byte is
/// killed and counted as a failure — a hung daemon must not hold its
/// dependents forever.
const READY_TIMEOUT_S: u64 = 60;
/// SIGTERM -> SIGKILL escalation window on stop.
const STOP_KILL_MS: u64 = 2_000;
const TICK_MS: u64 = 200;
/// M20c watchdog: timeout held on /dev/watchdog, and pet cadence (12x
/// margin, so a busy loop may skip pets without tripping the dog).
const WDT_TIMEOUT_S: i32 = 180;
const WDT_PET_MS: u64 = 15_000;

static TERM: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_sig: i32) {
    TERM.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    /// cmd missing on disk (pre-provisioning); re-check periodically.
    Absent,
    /// Waiting out the restart backoff (or a weak-dep hold).
    Backoff,
    /// Spawned, readiness not yet established.
    Starting,
    Ready,
    /// Parked by the circuit breaker or a non-restartable exit.
    Failed,
    /// SIGTERM sent, waiting for exit (stop_target decides what follows).
    Stopping,
    /// Stopped by `aginx-svc stop`.
    Stopped,
    /// Oneshot that exited 0.
    Done,
}

impl St {
    fn name(self) -> &'static str {
        match self {
            St::Absent => "absent",
            St::Backoff => "backoff",
            St::Starting => "starting",
            St::Ready => "ready",
            St::Failed => "failed",
            St::Stopping => "stopping",
            St::Stopped => "stopped",
            St::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterStop {
    StayStopped,
    Restart,
}

struct Run {
    unit: Unit,
    st: St,
    child: Option<Child>,
    /// Notify-type read end, kept for the child's lifetime (closing it
    /// would SIGPIPE the child on a later write; the readiness byte is
    /// consumed by poll, the eventual EOF ignored).
    notify: Option<File>,
    spawned: Option<Instant>,
    ready: Option<Instant>,
    backoff_until: Option<Instant>,
    backoff_ms: u64,
    absent_check: Option<Instant>,
    stopping_since: Option<Instant>,
    stop_target: AfterStop,
    fails: Vec<Instant>,
    /// Spawn count (1 = first start) — displayed as the unit's restarts.
    spawns: u32,
    last_exit: Option<i32>,
}

impl Run {
    fn new(u: Unit, now: Instant) -> Run {
        let st = if u.autostart {
            St::Backoff
        } else {
            St::Stopped
        };
        Run {
            unit: u,
            st,
            child: None,
            notify: None,
            spawned: None,
            ready: None,
            backoff_until: if st == St::Backoff { Some(now) } else { None },
            backoff_ms: 0,
            absent_check: None,
            stopping_since: None,
            stop_target: AfterStop::StayStopped,
            fails: Vec::new(),
            spawns: 0,
            last_exit: None,
        }
    }

    fn pid(&self) -> i32 {
        self.child.as_ref().map(|c| c.id() as i32).unwrap_or(-1)
    }

    fn set_st(&mut self, st: St) {
        if self.st != st {
            self.st = st;
            kmsg(&format!("aginx-svcd: {} {}\n", self.unit.name, st.name()));
        }
    }
}

fn spawn_unit(u: &Unit) -> Result<(Child, Option<File>), String> {
    if !path_exists(&u.cmd) {
        return Err("absent".into());
    }
    let mut cmd = Command::new(&u.cmd);
    cmd.args(&u.args)
        .env_clear()
        .env("PATH", "/sbin:/bin:/usr/sbin:/usr/bin:/var/bin")
        .env("HOME", "/home")
        .current_dir("/")
        .stdin(Stdio::null());
    for e in &u.envs {
        if let Some((k, v)) = e.split_once('=') {
            cmd.env(k, v);
        }
    }
    // env_file is re-read at every spawn so a re-provisioned key file
    // applies on the next restart without a reboot.
    if let Some(f) = &u.env_file {
        if let Ok(s) = std::fs::read_to_string(f) {
            for line in s.lines() {
                let l = line.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = l.split_once('=') {
                    cmd.env(k, v);
                }
            }
        }
    }
    std::fs::create_dir_all(LOG_DIR).ok();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(&u.name))
        .map_err(|e| format!("open log: {e}"))?;
    cmd.stdout(log.try_clone().map_err(|e| format!("dup log: {e}"))?);
    cmd.stderr(Stdio::from(log));

    let notify = if u.ty == SvcType::Notify {
        let mut pfd = [0 as RawFd; 2];
        if unsafe { libc::pipe(pfd.as_mut_ptr()) } != 0 {
            return Err(format!("pipe: {}", std::io::Error::last_os_error()));
        }
        // O_CLOEXEC is deliberately NOT set on this pipe: the write end
        // must survive exec, arriving on fd 3.
        let rd = unsafe { File::from_raw_fd(pfd[0]) };
        let wr_fd = pfd[1];
        cmd.env("AGINX_SVC_NOTIFY", "3");
        // SAFETY: the parent is single-threaded (no other threads exist
        // between fork and exec), and only async-signal-safe calls run
        // here (dup2/close/prctl).
        unsafe {
            cmd.pre_exec(move || {
                if wr_fd != 3 {
                    if libc::dup2(wr_fd, 3) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::close(wr_fd); // the dup2'd copy on fd 3 stays
                }
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong);
                Ok(())
            });
        }
        Some(rd)
    } else {
        unsafe {
            cmd.pre_exec(move || {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong);
                Ok(())
            });
        }
        None
    };

    let child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    Ok((child, notify))
}

struct Svc {
    runs: BTreeMap<String, Run>,
    listener: UnixListener,
}

impl Svc {
    // State helpers (name-keyed, so callers can release borrows before
    // chaining into other &mut self methods).

    fn set_st(&mut self, name: &str, st: St) {
        if let Some(r) = self.runs.get_mut(name) {
            r.set_st(st);
        }
    }

    /// Clear a pending restart target and schedule an immediate respawn.
    fn respawn_now(&mut self, name: &str, now: Instant) {
        if let Some(r) = self.runs.get_mut(name) {
            r.stop_target = AfterStop::StayStopped;
            r.backoff_until = Some(now);
            r.set_st(St::Backoff);
        }
    }

    fn try_spawn(&mut self, name: &str, now: Instant) {
        // weak-dep gate: hold while a required unit is still starting
        // (§12.1 — readiness blocking is the ordering; any other state of
        // the dep — absent/failed/ready/stopped — lets us through)
        let (deps, unit) = {
            let Some(r) = self.runs.get(name) else { return };
            if r.st != St::Backoff {
                return;
            }
            (r.unit.requires_weak.clone(), r.unit.clone())
        };
        for dep in &deps {
            if let Some(d) = self.runs.get(dep) {
                if d.st == St::Starting {
                    if let Some(r) = self.runs.get_mut(name) {
                        r.backoff_until = Some(now + Duration::from_millis(DEP_WAIT_MS));
                    }
                    return;
                }
            }
        }
        match spawn_unit(&unit) {
            Ok((child, notify)) => {
                if let Some(r) = self.runs.get_mut(name) {
                    r.child = Some(child);
                    r.notify = notify;
                    r.spawned = Some(now);
                    r.ready = None;
                    r.stopping_since = None;
                    r.spawns = r.spawns.saturating_add(1);
                    r.set_st(St::Starting);
                }
            }
            Err(e) if e == "absent" => {
                if let Some(r) = self.runs.get_mut(name) {
                    r.absent_check = Some(now + Duration::from_millis(ABSENT_RECHECK_MS));
                    r.set_st(St::Absent);
                }
            }
            Err(e) => {
                kmsg(&format!("aginx-svcd: {} spawn failed: {e}\n", unit.name));
                self.failure(name, now, &e);
            }
        }
    }

    fn failure(&mut self, name: &str, now: Instant, why: &str) {
        let Some(r) = self.runs.get_mut(name) else { return };
        r.fails.push(now);
        while let Some(&f) = r.fails.first() {
            if now.duration_since(f).as_secs() >= BREAKER_WINDOW_S {
                r.fails.remove(0);
            } else {
                break;
            }
        }
        r.backoff_ms = if r.backoff_ms == 0 {
            BACKOFF_START_MS
        } else {
            (r.backoff_ms * 2).min(BACKOFF_MAX_MS)
        };
        let park = r.fails.len() >= BREAKER_N;
        let restartable = r.unit.restart;
        let display = r.unit.name.clone();
        r.child = None;
        r.notify = None;
        if park {
            kmsg(&format!(
                "aginx-svcd: {display} breaker open ({why}, {} exits in {BREAKER_WINDOW_S}s)\n",
                r.fails.len()
            ));
            r.set_st(St::Failed);
        } else if restartable {
            r.backoff_until = Some(now + Duration::from_millis(r.backoff_ms));
            r.set_st(St::Backoff);
        } else {
            r.set_st(St::Failed);
        }
    }

    fn on_exit(&mut self, name: &str, exit: i32, now: Instant) {
        enum Act {
            Done,
            Fail(String),
            Respawn,
            ToStopped,
        }
        let act = {
            let Some(r) = self.runs.get_mut(name) else { return };
            r.child = None;
            r.notify = None;
            r.last_exit = Some(exit);
            let uptime = r
                .spawned
                .map(|t| now.duration_since(t).as_secs())
                .unwrap_or(0);
            match r.st {
                St::Starting => {
                    if r.unit.is_oneshot() && exit == 0 {
                        Act::Done
                    } else {
                        Act::Fail("exited before ready".into())
                    }
                }
                St::Ready => {
                    if r.stop_target == AfterStop::Restart {
                        Act::Respawn
                    } else if r.unit.is_oneshot() && exit == 0 {
                        Act::Done
                    } else {
                        if uptime >= STABLE_S {
                            // ran stably — restart with fresh backoff, but
                            // the exit still counts toward the breaker
                            r.backoff_ms = 0;
                        }
                        Act::Fail(format!("exit {exit} after ready"))
                    }
                }
                St::Stopping => {
                    if r.stop_target == AfterStop::Restart {
                        Act::Respawn
                    } else {
                        Act::ToStopped
                    }
                }
                _ => Act::ToStopped,
            }
        };
        match act {
            Act::Done => self.set_st(name, St::Done),
            Act::Fail(w) => self.failure(name, now, &w),
            Act::Respawn => self.respawn_now(name, now),
            Act::ToStopped => self.set_st(name, St::Stopped),
        }
    }

    fn ready(&mut self, name: &str, now: Instant) {
        if let Some(r) = self.runs.get_mut(name) {
            if r.st != St::Starting {
                return;
            }
            r.ready = Some(now);
            r.backoff_ms = 0;
            r.set_st(St::Ready);
        }
    }

    fn stop(&mut self, name: &str, target: AfterStop, now: Instant) {
        let Some(r) = self.runs.get_mut(name) else { return };
        match r.st {
            St::Ready | St::Starting => {
                r.stop_target = target;
                r.stopping_since = Some(now);
                r.set_st(St::Stopping);
                if let Some(c) = &r.child {
                    unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
                }
            }
            St::Backoff => {
                r.child = None;
                match target {
                    AfterStop::Restart => r.backoff_until = Some(now),
                    AfterStop::StayStopped => r.set_st(St::Stopped),
                }
            }
            St::Absent | St::Failed | St::Stopped | St::Done => {
                if target == AfterStop::Restart {
                    r.fails.clear();
                    r.backoff_ms = 0;
                    r.backoff_until = Some(now);
                    r.set_st(St::Backoff);
                }
            }
            St::Stopping => {
                r.stop_target = target;
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        let names: Vec<String> = self.runs.keys().cloned().collect();
        for name in names {
            // Absent→Backoff pre-pass: try_spawn only acts on Backoff, so
            // a due absent-recheck must flip the state first or it spins
            // forever without ever reaching spawn_unit (found on
            // first-boot provisioning, 2026-08-31: units stayed absent
            // 25 min after /var/bin filled).
            if let Some(r) = self.runs.get_mut(&name) {
                if r.st == St::Absent && r.absent_check.map(|t| now >= t).unwrap_or(false) {
                    r.set_st(St::Backoff);
                    r.backoff_until = Some(now);
                }
            }
            enum Act {
                Spawn,
                Ready,
                Kill, // stuck before ready, or TERM ignored past the window
            }
            let act = {
                let Some(r) = self.runs.get(&name) else { continue };
                match r.st {
                    St::Backoff => {
                        if r.backoff_until.map(|t| now >= t).unwrap_or(false) {
                            Some(Act::Spawn)
                        } else {
                            None
                        }
                    }
                    St::Starting => {
                        let e = now.duration_since(r.spawned.unwrap_or(now));
                        match r.unit.ty {
                            SvcType::Simple if e >= Duration::from_millis(SIMPLE_GRACE_MS) => {
                                Some(Act::Ready)
                            }
                            SvcType::Socket if path_exists(&socket_for(&r.unit.name)) => {
                                Some(Act::Ready)
                            }
                            _ if e >= Duration::from_secs(READY_TIMEOUT_S) => Some(Act::Kill),
                            _ => None,
                        }
                    }
                    St::Stopping => {
                        let since = r.stopping_since.unwrap_or(now);
                        if now.duration_since(since) >= Duration::from_millis(STOP_KILL_MS) {
                            Some(Act::Kill)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            };
            match act {
                Some(Act::Spawn) => self.try_spawn(&name, now),
                Some(Act::Ready) => self.ready(&name, now),
                Some(Act::Kill) => {
                    if let Some(r) = self.runs.get(&name) {
                        if let Some(c) = &r.child {
                            unsafe { libc::kill(c.id() as i32, libc::SIGKILL) };
                        }
                    }
                }
                None => {}
            }
        }
    }

    fn reap(&mut self, now: Instant) {
        let mut exited: Vec<(String, i32)> = Vec::new();
        for (name, r) in self.runs.iter_mut() {
            let Some(c) = r.child.as_mut() else { continue };
            match c.try_wait() {
                Ok(Some(status)) => {
                    let code = status
                        .code()
                        .or_else(|| status.signal().map(|s| -s))
                        .unwrap_or(0);
                    exited.push((name.clone(), code));
                }
                Ok(None) => {}
                Err(_) => exited.push((name.clone(), -999)),
            }
        }
        for (name, code) in exited {
            self.on_exit(&name, code, now);
        }
    }

    fn read_notify(&mut self, name: &str) {
        let got = {
            let Some(r) = self.runs.get_mut(name) else { return };
            if r.st != St::Starting {
                return;
            }
            let mut buf = [0u8; 1];
            matches!(
                r.notify.as_mut().map(|rd| rd.read(&mut buf)),
                Some(Ok(1))
            )
        };
        if got {
            self.ready(name, Instant::now());
        }
    }

    // ---------------------------------------------------------------- ctl

    fn ctl(&mut self, line: &str) -> Vec<String> {
        let now = Instant::now();
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").to_string();
        match cmd {
            "list" => {
                let mut out = Vec::new();
                for r in self.runs.values() {
                    out.push(format!(
                        "{}\t{}\t{}\t{}\t{}",
                        r.unit.name,
                        r.st.name(),
                        r.pid(),
                        r.spawns,
                        r.unit.cmd
                    ));
                }
                out.push("OK".into());
                out
            }
            "status" => {
                let Some(r) = self.runs.get(&arg) else {
                    return vec![format!("ERR no such unit: {arg}")];
                };
                vec![
                    format!("name    {}", r.unit.name),
                    format!("state   {}", r.st.name()),
                    format!("pid     {}", r.pid()),
                    format!("type    {:?}", r.unit.ty),
                    format!("cmd     {} {}", r.unit.cmd, r.unit.args.join(" ")),
                    format!("restart {} (backoff {} ms)", r.unit.restart, r.backoff_ms),
                    format!(
                        "exits   {} in breaker window, last exit {:?}",
                        r.fails.len(),
                        r.last_exit
                    ),
                    format!("spawns  {}", r.spawns),
                    format!("weak    {}", r.unit.requires_weak.join(" ")),
                    format!("unit    {}", r.unit.src_path),
                    format!("log     {}", log_path(&r.unit.name)),
                    "OK".into(),
                ]
            }
            "start" => {
                let Some(r) = self.runs.get_mut(&arg) else {
                    return vec![format!("ERR no such unit: {arg}")];
                };
                match r.st {
                    St::Ready | St::Starting => vec![format!("OK already {}", r.st.name())],
                    St::Stopping => vec!["ERR stopping, retry after it exits".into()],
                    _ => {
                        r.fails.clear();
                        r.backoff_ms = 0;
                        r.backoff_until = Some(now);
                        r.set_st(St::Backoff);
                        vec!["OK".into()]
                    }
                }
            }
            "stop" => {
                if !self.runs.contains_key(&arg) {
                    return vec![format!("ERR no such unit: {arg}")];
                }
                self.stop(&arg, AfterStop::StayStopped, now);
                vec!["OK".into()]
            }
            "restart" => {
                if !self.runs.contains_key(&arg) {
                    return vec![format!("ERR no such unit: {arg}")];
                }
                self.stop(&arg, AfterStop::Restart, now);
                vec!["OK".into()]
            }
            "reload" => self.reload(now),
            _ => vec![
                "ERR usage: list | status <name> | start|stop|restart <name> | reload".into(),
            ],
        }
    }

    /// Rescan unit dirs: add new units, drop removed ones, restart units
    /// whose file changed (byte compare). agpkg installs land here.
    fn reload(&mut self, now: Instant) -> Vec<String> {
        let fresh = load_units(&UNIT_DIRS);
        let fresh_names: std::collections::HashSet<String> =
            fresh.iter().map(|u| u.name.clone()).collect();
        let mut out = Vec::new();
        let mut respawns: Vec<String> = Vec::new();
        for u in fresh {
            match self.runs.get_mut(&u.name) {
                None => {
                    out.push(format!("+ {}", u.name));
                    let name = u.name.clone();
                    let autostart = u.autostart;
                    self.runs.insert(name.clone(), Run::new(u, now));
                    if autostart {
                        self.try_spawn(&name, now);
                    }
                }
                Some(r) => {
                    if r.unit.src_bytes != u.src_bytes {
                        out.push(format!("~ {}", u.name));
                        let was_active =
                            matches!(r.st, St::Ready | St::Starting | St::Backoff);
                        r.unit = u;
                        if was_active {
                            respawns.push(r.unit.name.clone());
                        }
                    }
                }
            }
        }
        for name in self.runs.keys().cloned().collect::<Vec<_>>() {
            if !fresh_names.contains(&name) {
                out.push(format!("- {name}"));
                self.stop(&name, AfterStop::StayStopped, now);
                self.runs.remove(&name);
            }
        }
        for name in respawns {
            self.stop(&name, AfterStop::Restart, now);
        }
        out.push("OK".into());
        out
    }
}

fn setup_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, on_term as *const () as usize);
        libc::signal(libc::SIGINT, on_term as *const () as usize);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

fn next_deadline(runs: &BTreeMap<String, Run>, now: Instant) -> Option<Instant> {
    runs.values()
        .filter_map(|r| match r.st {
            St::Backoff => r.backoff_until,
            St::Absent => r.absent_check,
            St::Starting => r.spawned.map(|t| {
                let grace = match r.unit.ty {
                    SvcType::Simple => Duration::from_millis(SIMPLE_GRACE_MS),
                    _ => Duration::from_secs(READY_TIMEOUT_S),
                };
                t + grace
            }),
            St::Stopping => r
                .stopping_since
                .map(|t| t + Duration::from_millis(STOP_KILL_MS)),
            _ => None,
        })
        .min()
        .map(|d| d.max(now))
}

// M20c: software-watchdog heartbeat. /dev/watchdog on this platform is
// softdog behind the watchdog_v2 (platform:msm_watchdog) driver — dmesg
// says "wdog absent resource not present" (the APPS-side hardware bark
// resources are missing from this DT, which is why the M14 bad-kernel
// hang sat dark with no rescue), and the 2026-09-02 starve test proved
// the soft dog still hard-resets an unpetted box. Contract: aginx-svcd pets
// from its supervision loop, so "supervisor alive" keeps the box up and
// a wedged supervisor (STOP, deadlock, a runaway unit starving the
// loop) resets it — ABL drains a boot try and M14's rollback takes
// over. A hung KERNEL is outside this dog's reach (the timer dies with
// it); that class is fenced by aginx-update's verify-before-write instead.
// Opening arms the dog for the life of the kernel (nowayout — the
// starve test never closed and still reset), which is what we want:
// there is no legitimate state where aginx-svcd stops petting.
struct Wdt {
    fd: Option<RawFd>,
    last: Instant,
    warned: bool,
}

// <linux/watchdog.h> numbers (no libc crate bindings for these). NB:
// SETTIMEOUT is _IOWR (0xC0.., the driver writes back the timeout it
// accepted) — a _IOW guess gets ENOTTY from this driver, measured
// 2026-09-02. KEEPALIVE is _IOR. c_int because musl's ioctl takes the
// request as int (unlike glibc's unsigned long).
const WDIOC_SETTIMEOUT: libc::c_int = 0xC004_5706u32 as libc::c_int;
const WDIOC_KEEPALIVE: libc::c_int = 0x8004_5705u32 as libc::c_int;

impl Wdt {
    fn new() -> Self {
        Wdt {
            fd: None,
            last: Instant::now(),
            warned: false,
        }
    }

    // Lazy open: watchdog_v2 loads during the vendor module pass, which
    // races our start — retry quietly on each pet until the node appears.
    fn arm(&mut self) {
        let path = b"/dev/watchdog\0";
        let fd = unsafe {
            libc::open(path.as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CLOEXEC)
        };
        if fd < 0 {
            if !self.warned {
                kmsg("aginx-svcd: wdt: /dev/watchdog not there yet — petting starts when it appears\n");
                self.warned = true;
            }
            return;
        }
        let mut t = WDT_TIMEOUT_S;
        let rc = unsafe { libc::ioctl(fd, WDIOC_SETTIMEOUT, &mut t as *mut libc::c_int) };
        match rc {
            0 => kmsg(&format!("aginx-svcd: wdt armed, timeout={t}s, petting every {}s\n", WDT_PET_MS / 1000)),
            _ => {
                let e = std::io::Error::last_os_error();
                kmsg(&format!("aginx-svcd: wdt: SETTIMEOUT: {e} (continuing armed at driver default)\n"));
            }
        }
        self.fd = Some(fd);
    }

    fn pet(&mut self, now: Instant) {
        if self.fd.is_none() {
            self.arm();
            if self.fd.is_none() {
                return;
            }
        }
        if now.duration_since(self.last).as_millis() < WDT_PET_MS as u128 {
            return;
        }
        self.last = now;
        let fd = self.fd.unwrap();
        if unsafe { libc::ioctl(fd, WDIOC_KEEPALIVE, 0) } < 0 && !self.warned {
            let e = std::io::Error::last_os_error();
            kmsg(&format!("aginx-svcd: wdt: keepalive: {e}\n"));
            self.warned = true;
        }
    }
}

fn main() {
    setup_signals();
    std::fs::create_dir_all("/run/aginx-svc").ok();
    std::fs::create_dir_all(LOG_DIR).ok();
    let _ = std::fs::remove_file(CTL_SOCK);
    let listener = match UnixListener::bind(CTL_SOCK) {
        Ok(l) => l,
        Err(e) => {
            kmsg(&format!("aginx-svcd: bind {CTL_SOCK}: {e}\n"));
            std::process::exit(1);
        }
    };
    let _ = listener.set_nonblocking(true);
    let _ = std::fs::set_permissions(CTL_SOCK, Permissions::from_mode(0o600));
    if let Ok(mut f) = File::create("/run/aginx-svcd.pid") {
        let _ = writeln!(f, "{}", std::process::id());
    }

    let now = Instant::now();
    let mut svc = Svc {
        runs: BTreeMap::new(),
        listener,
    };
    let units = load_units(&UNIT_DIRS);
    let n = units.len();
    for u in units {
        let name = u.name.clone();
        let autostart = u.autostart;
        svc.runs.insert(name.clone(), Run::new(u, now));
        if autostart {
            svc.try_spawn(&name, now);
        }
    }
    kmsg(&format!("aginx-svcd: supervisor up, {n} units\n"));
    let mut wdt = Wdt::new();

    loop {
        if TERM.load(Ordering::SeqCst) {
            break;
        }
        let now = Instant::now();

        // poll: the ctl listener + notify pipes of starting units
        let mut notify_fds: Vec<String> = Vec::new();
        let mut fds = vec![libc::pollfd {
            fd: svc.listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        for (name, r) in svc.runs.iter() {
            if r.st == St::Starting {
                if let Some(rd) = &r.notify {
                    notify_fds.push(name.clone());
                    fds.push(libc::pollfd {
                        fd: rd.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    });
                }
            }
        }
        let timeout = match next_deadline(&svc.runs, now) {
            Some(d) => (d.duration_since(now).as_millis() as u64 + 1).min(TICK_MS),
            None => TICK_MS,
        }
        .max(10) as i32;
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if rc < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                kmsg(&format!("aginx-svcd: poll: {e}\n"));
            }
        }
        if fds[0].revents & libc::POLLIN != 0 {
            loop {
                match svc.listener.accept() {
                    Ok((stream, _)) => handle_conn(stream, &mut svc),
                    Err(_) => break,
                }
            }
        }
        let mut readied: Vec<String> = Vec::new();
        for (i, name) in notify_fds.iter().enumerate() {
            if fds[i + 1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                readied.push(name.clone());
            }
        }
        for name in readied {
            svc.read_notify(&name);
        }
        svc.reap(Instant::now());
        svc.tick(Instant::now());
        wdt.pet(Instant::now());
    }

    // shutdown: TERM everyone, escalate, exit. Reboot takes the machine
    // anyway; this path exists so a controlled stop doesn't leak daemons.
    let pids: Vec<i32> = svc
        .runs
        .values()
        .filter_map(|r| r.child.as_ref().map(|c| c.id() as i32))
        .collect();
    for p in &pids {
        unsafe { libc::kill(*p, libc::SIGTERM) };
    }
    if !pids.is_empty() {
        std::thread::sleep(Duration::from_millis(300));
        for p in &pids {
            unsafe { libc::kill(*p, libc::SIGKILL) };
        }
    }
    kmsg("aginx-svcd: down\n");
    let _ = std::fs::remove_file(CTL_SOCK);
}

fn handle_conn(mut stream: UnixStream, svc: &mut Svc) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 4096 || buf.contains(&b'\n') {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let line = String::from_utf8_lossy(&buf).trim().to_string();
    if line.is_empty() {
        return;
    }
    // SO_PEERCRED on every connection (§12.2 CallerCtx): while everything
    // is root this only audits, but non-root ctl use gets logged.
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as u32;
    let cred_ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        ) == 0
    };
    let resp = svc.ctl(&line).join("\n") + "\n";
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
    if cred_ok && cred.uid != 0 {
        kmsg(&format!(
            "aginx-svcd: ctl '{line}' from uid {} pid {}\n",
            cred.uid, cred.pid
        ));
    }
}
