//! aginx-pair apply — 配对载荷的机器侧执行（C3，蛋的配网手）。
//!
//! 零位置参数，payload 一行从 **stdin** 读（`AGINXPAIR1|…` 全身份码或
//! `WIFI:…` 连网码）：argv 恒两词，/proc/*/cmdline 永不出现 psk/三键——
//! argv 泄密是硬红线。五步照搬 voice 的 pair_apply（M42c 一眼自举）：
//! ①join（spawn aginx-net-join，90s 预算 + 10×500ms IP 轮询，成功才落
//! /etc/wifi.conf 0600 tmp+rename）②env 三键合并（保旧行）③快速校时
//! ④internet 探测 + 母体两单元 restart-ready ⑤一行汇总。`WIFI:` 码只走
//! ①+④探测——没有身份可灌。
//!
//! 秘密卫生：stdout 只出一行汇总（无任何字段值）；stderr 记步骤名，ssid
//! 可记、psk/三键永不。退出码 0 成功 / 1 步骤失败 / 2 坏 payload。
//!
//! 成功后**定点刷新 /run/boot.state** 的网四行（wifi/dhcp/internet/time）：
//! 蛋首启 provision 已因 `wifi fail` 早退，没人会再写这些行——term 的等网
//! 行、清单面的自动 sync 门、n6 断言都读它。幂等：重跑同值。
//!
//! 外部触点全部 env 可覆写（AGINX_PAIR_NET_JOIN/SVC/HTTPGET/NTPD/IP/
//! WIFI_CONF/ENV/STATE/IFACE——pkg crate 的 envp 律）：设备上没人设它们，
//! host 测试直接构造 PairPaths 指向一树 stub，不碰进程 env。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use aginx_qr::{parse_pair_payload, parse_wifi_payload, PairBundle};

const JOIN_BUDGET_SECS: u32 = 90;
/// udhcpc -n -q -t 10 -T 3 = 30s 最坏重试 + 余量（net-bringup 同款参数）。
const DHCP_BUDGET_SECS: u32 = 40;

/// 外部触点集。from_env 在设备上给出常量真值；测试直接构造。
#[derive(Debug, Clone)]
pub struct PairPaths {
    pub net_join: PathBuf,
    pub udhcpc: PathBuf,
    pub svc: PathBuf,
    pub httpget: PathBuf,
    pub ntpd: PathBuf,
    pub ip: PathBuf,
    pub wifi_conf: PathBuf,
    pub env_file: PathBuf,
    pub state: PathBuf,
    pub iface: String,
}

fn envp(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default))
}

impl PairPaths {
    pub fn from_env() -> PairPaths {
        PairPaths {
            net_join: envp("AGINX_PAIR_NET_JOIN", "/usr/bin/aginx-net-join"),
            udhcpc: envp("AGINX_PAIR_UDHCPC", "/bin/udhcpc"),
            svc: envp("AGINX_PAIR_SVC", "/usr/bin/aginx-svc"),
            httpget: envp("AGINX_PAIR_HTTPGET", "/bin/httpget"),
            ntpd: envp("AGINX_PAIR_NTPD", "ntpd"),
            ip: envp("AGINX_PAIR_IP", "ip"),
            wifi_conf: envp("AGINX_PAIR_WIFI_CONF", "/etc/wifi.conf"),
            env_file: envp("AGINX_PAIR_ENV", "/etc/aginx/env"),
            state: envp("AGINX_PAIR_STATE", "/run/boot.state"),
            iface: std::env::var("AGINX_PAIR_IFACE").unwrap_or_else(|_| "wlan0".into()),
        }
    }
}

/// run_with 的失败两分：坏 payload 是用法错（exit 2），步骤失败是运行错
/// （exit 1）——provision/term 只看退出码。
#[derive(Debug, PartialEq)]
pub enum ApplyErr {
    BadPayload,
    Step(String),
}

/// CLI 入口：stdin 一行 → 执行 → 汇总行到 stdout。返回退出码。
pub fn run(p: &PairPaths) -> i32 {
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() || line.trim().is_empty() {
        eprintln!("aginx-pair: apply wants one payload line on stdin");
        return 2;
    }
    match run_with(p, line.trim()) {
        Ok(summary) => {
            println!("{summary}");
            0
        }
        Err(ApplyErr::BadPayload) => {
            eprintln!("aginx-pair: payload is neither AGINXPAIR1|… nor WIFI:…");
            2
        }
        Err(ApplyErr::Step(m)) => {
            eprintln!("aginx-pair: {m}");
            1
        }
    }
}

/// 一行 payload 进、汇总行出。成功路径里完成 boot.state 刷新（见模块注）。
pub fn run_with(p: &PairPaths, line: &str) -> Result<String, ApplyErr> {
    if let Some(b) = parse_pair_payload(line) {
        apply_pair(p, &b)
    } else if let Some(w) = parse_wifi_payload(line) {
        apply_wifi(p, &w.ssid, &w.psk)
    } else {
        Err(ApplyErr::BadPayload)
    }
}

/// 全身份码：join → env 三键 → 校时 → internet 探测 → 母体两单元。
fn apply_pair(p: &PairPaths, b: &PairBundle) -> Result<String, ApplyErr> {
    let step = |m: String| ApplyErr::Step(m);
    let ip = join_wifi(p, &b.ssid, &b.psk).map_err(step)?;
    eprintln!("aginx-pair: joined ssid={} ip={ip}", b.ssid);
    write_env_keys(
        p,
        &[
            ("AGINXBRAIN_API_KEY", &b.brain_key),
            ("AGINX_GATEWAY_ID", &b.gateway_id),
            ("AGINX_RELAY_SECRET", &b.relay_secret),
        ],
    )
    .map_err(step)?;
    eprintln!("aginx-pair: env keys merged (3)");
    let clock_ok = quick_clock(p);
    let net_ok = internet_probe(p);
    let up = svc_ready_after_restart(p, "aginx-gateway") && svc_ready_after_restart(p, "aginx-server");
    refresh_state(p, &b.ssid, Some(&ip), net_ok, if clock_ok { Some(true) } else { None });
    let mut msg = String::from("网已连");
    if !clock_ok {
        msg.push_str("，时钟没同步");
    }
    msg.push_str(if up { "，母体在线" } else { "，母体没起来" });
    Ok(msg)
}

/// WIFI: 码：连网 + 探测，不灌身份不起单元。
fn apply_wifi(p: &PairPaths, ssid: &str, psk: &str) -> Result<String, ApplyErr> {
    let ip = join_wifi(p, ssid, psk).map_err(ApplyErr::Step)?;
    eprintln!("aginx-pair: joined ssid={ssid} ip={ip}");
    let net_ok = internet_probe(p);
    refresh_state(p, ssid, Some(&ip), net_ok, None);
    Ok(format!("网已连 {ssid}"))
}

/// net-join 只装钥匙（wifi-join.c：keys installed — run udhcpc）；租约是
/// udhcpc 的活——net-bringup/net-rejoin 一直这么分。配网路径此前裸奔：
/// 关联成而 IP 永不来（2026-09-09 蛋首配收据，assoc UP 而 inet 空）。
/// 成功才落 wifi.conf；坏密钥不落盘（wizard 撤回语义）。
fn join_wifi(p: &PairPaths, ssid: &str, psk: &str) -> Result<String, String> {
    let mut child = Command::new(&p.net_join)
        .args([&p.iface, ssid, psk])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("net-join spawn: {e}"))?;
    wait_limited(&mut child, JOIN_BUDGET_SECS).map_err(|e| format!("net-join {e}"))?;
    // 租约腿（net-bringup 同款参数：-n 租到即退，10×3s 重试）。地址已在
    // （开机路径的 udhcpc 先跑过）就跳过，幂等不重试。
    if iface_ip(p).is_none() {
        let mut dhcp = Command::new(&p.udhcpc)
            .args(["-i", &p.iface, "-n", "-q", "-t", "10", "-T", "3"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("udhcpc spawn: {e}"))?;
        wait_limited(&mut dhcp, DHCP_BUDGET_SECS).map_err(|e| format!("udhcpc {e}"))?;
    }
    for _ in 0..10 {
        if let Some(ip) = iface_ip(p) {
            persist_wifi(p, ssid, psk);
            return Ok(ip);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("没拿到地址".into())
}

/// `ip -4 addr show <iface>` 的第一个非环回 IPv4。
fn iface_ip(p: &PairPaths) -> Option<String> {
    let out = Command::new(&p.ip).args(["-4", "addr", "show", &p.iface]).output().ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let Some(ip) = rest.split_whitespace().next() {
                // real output carries the prefix length: 192.168.1.42/24
                let bare = ip.split('/').next().unwrap_or(ip);
                if bare != "127.0.0.1" && !bare.is_empty() {
                    return Some(bare.to_string());
                }
            }
        }
    }
    None
}

/// 连上网后落 wifi.conf：0600、tmp+rename（voice persist_wifi 同法）。失败
/// 不致命到翻转 join 结果——只 stderr（盘上没有身份，下次 bringup 会再试）。
fn persist_wifi(p: &PairPaths, ssid: &str, psk: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    let conf = format!("ssid={ssid}\npsk={psk}\n");
    let tmp = p.wifi_conf.with_extension("conf.tmp");
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(conf.as_bytes()))
        .is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, &p.wifi_conf);
    } else {
        eprintln!("aginx-pair: persist wifi.conf failed");
    }
}

/// 身份键并入 env 文件（KEY=VALUE、# 注释——svc spawn 重读的同一形状）。
/// 保留既有行，同名键原地替换，缺的尾部追加。0600 tmp+rename。
fn write_env_keys(p: &PairPaths, kvs: &[(&str, &str)]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let existing = std::fs::read_to_string(&p.env_file).unwrap_or_default();
    let has_key = |k: &str| {
        existing
            .lines()
            .any(|l| l.split_once('=').map(|(ek, _)| ek == k).unwrap_or(false))
    };
    let mut out = String::new();
    for line in existing.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        match line.split_once('=') {
            Some((k, _)) if kvs.iter().any(|(nk, _)| *nk == k) => {
                let v = &kvs.iter().find(|(nk, _)| *nk == k).unwrap().1;
                out.push_str(&format!("{k}={v}\n"));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (k, v) in kvs {
        if !has_key(k) {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    let tmp = p.env_file.with_extension("tmp");
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(out.as_bytes()))
        .map_err(|e| format!("env write: {e}"))?;
    std::fs::rename(&tmp, &p.env_file).map_err(|e| format!("env rename: {e}"))
}

/// 快速校时（voice quick_clock 同款）：双 NTP ×10s 交替，`date +%Y≥2026`
/// 判定。失败不致命——进汇总话，net-watch/下次 bringup 会补。
fn quick_clock(p: &PairPaths) -> bool {
    for server in ["ntp.aliyun.com", "cn.pool.ntp.org"] {
        if let Ok(mut child) = Command::new(&p.ntpd)
            .args(["-q", "-n", "-p", server])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let _ = wait_limited(&mut child, 10);
            let _ = child.wait();
        }
        let ok = Command::new("date")
            .arg("+%Y")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .map(|y| y >= 2026)
            .unwrap_or(false);
        if ok {
            return true;
        }
    }
    false
}

/// internet 探测（net-bringup 同法）：/bin/httpget 拉 baidu——busybox wget
/// 段错误，httpget 是蛋里在册的取件人。
fn internet_probe(p: &PairPaths) -> bool {
    Command::new(&p.httpget)
        .args(["http://www.baidu.com/", "/tmp/aginx-pair-probe.html"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// restart 一个 unit 并回查到 ready（voice svc_ready_after_restart 同款：
/// simple 型 spawn 即 ready；熔断 failed 单元 restart 照样救活）。
fn svc_ready_after_restart(p: &PairPaths, unit: &str) -> bool {
    let st = Command::new(&p.svc)
        .args(["restart", unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !st.map(|s| s.success()).unwrap_or(false) {
        eprintln!("aginx-pair: svc restart {unit} failed");
        return false;
    }
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(o) = Command::new(&p.svc).args(["status", unit]).output() {
            let txt = String::from_utf8_lossy(&o.stdout);
            if txt.lines().any(|l| l.trim() == "state   ready") {
                return true;
            }
            if txt.lines().any(|l| l.trim() == "state   failed") {
                return false;
            }
        }
    }
    false
}

/// 定点刷新 boot.state 的网四行：已有行（run/fail/ok 任何态）原地改写为
/// 本次判词，缺行按 wifi→dhcp→internet→time 顺序补尾。time 只在真同步
/// 成功时写 ok（失败留给 net-bringup 语义，汇总话里说）。其他行不动。
fn refresh_state(p: &PairPaths, ssid: &str, ip: Option<&str>, internet: bool, time_ok: Option<bool>) {
    let mut verdicts: Vec<(&str, String)> = vec![("wifi", format!("wifi ok {ssid}"))];
    if let Some(ip) = ip {
        verdicts.push(("dhcp", format!("dhcp ok {ip}")));
    }
    verdicts.push((
        "internet",
        if internet { "internet ok paired".to_string() } else { "internet fail paired".to_string() },
    ));
    if time_ok == Some(true) {
        verdicts.push(("time", "time ok".to_string()));
    }
    let existing = std::fs::read_to_string(&p.state).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut wrote: Vec<&str> = Vec::new();
    for line in existing.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        match verdicts.iter().find(|(n, _)| *n == name) {
            Some((_, v)) => {
                out.push(v.clone());
                wrote.push(name);
            }
            None => out.push(line.to_string()),
        }
    }
    for (name, v) in &verdicts {
        if !wrote.contains(name) {
            out.push(v.clone());
        }
    }
    let body = format!("{}\n", out.join("\n"));
    if let Some(parent) = p.state.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p.state, body);
}

/// wait_limited（voice audio 同款）：预算内 try_wait 轮询，超时 kill。
fn wait_limited(child: &mut Child, secs: u32) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs as u64);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                return if st.success() { Ok(()) } else { Err(format!("exit {st}")) };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timeout".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use testkit::tmp;

    fn paths(root: &std::path::Path) -> PairPaths {
        // stub tree: every external touchpoint is a script we control
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let mk = |name: &str, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            let p = bin.join(name);
            fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        mk("net-join", "exit 0");
        mk("udhcpc", "exit 0");
        mk("ip", "echo '    inet 192.168.1.42/24 brd 192.168.1.255 scope global'");
        mk("ntpd", "exit 0"); // date +%Y on the host is the real verdict
        mk("httpget", "exit 0");
        mk("svc", r#"case "$1" in restart) exit 0;; status) echo "state   ready";; esac"#);
        PairPaths {
            net_join: bin.join("net-join"),
            udhcpc: bin.join("udhcpc"),
            svc: bin.join("svc"),
            httpget: bin.join("httpget"),
            ntpd: bin.join("ntpd"),
            ip: bin.join("ip"),
            wifi_conf: root.join("wifi.conf"),
            env_file: root.join("env"),
            state: root.join("boot.state"),
            iface: "wlan0".into(),
        }
    }

    const PAYLOAD: &str = "AGINXPAIR1|Legrand AP|p4ss w0rd!|sk-1234567890abcdef1234567890abcdef|cf49973e|relay-secret-9f8e7d6c";

    #[test]
    fn full_pair_flow_stubs_all_green() {
        let root = tmp("aginx-pair-apply-full");
        let p = paths(&root);
        fs::write(&p.env_file, "# identity\nHOME=/home/aginx\nOLDKEY=x\n").unwrap();
        fs::write(&p.state, "touch ok\ntime run\nwifi fail no /etc/wifi.conf\ndhcp fail\ninternet fail\n").unwrap();

        let msg = run_with(&p, PAYLOAD).unwrap();
        // summary carries no field values (ssid allowed, secrets never)
        assert!(msg.contains("网已连"), "{msg}");
        assert!(!msg.contains("p4ss"), "{msg}");
        assert!(!msg.contains("sk-1234"), "{msg}");
        assert!(!msg.contains("relay-secret"), "{msg}");

        // wifi.conf 0600 with both keys
        assert_eq!(fs::read_to_string(&p.wifi_conf).unwrap(), "ssid=Legrand AP\npsk=p4ss w0rd!\n");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&p.wifi_conf).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // env: comment + HOME preserved, three keys merged, 0600
        let env = fs::read_to_string(&p.env_file).unwrap();
        assert!(env.contains("# identity\n"));
        assert!(env.contains("HOME=/home/aginx\n"));
        assert!(env.contains("AGINXBRAIN_API_KEY=sk-1234567890abcdef1234567890abcdef\n"));
        assert!(env.contains("AGINX_GATEWAY_ID=cf49973e\n"));
        assert!(env.contains("AGINX_RELAY_SECRET=relay-secret-9f8e7d6c\n"));
        let emode = fs::metadata(&p.env_file).unwrap().permissions().mode();
        assert_eq!(emode & 0o777, 0o600);

        // boot.state: other lines kept, net verdicts rewritten/appended
        let state = fs::read_to_string(&p.state).unwrap();
        let lines: Vec<&str> = state.lines().collect();
        assert!(lines.contains(&"touch ok"));
        assert!(lines.contains(&"wifi ok Legrand AP"));
        assert!(lines.contains(&"dhcp ok 192.168.1.42"));
        assert!(lines.contains(&"internet ok paired"));
        assert!(lines.contains(&"time ok")); // host date is 2026+

        // idempotent re-run: same file, no duplicated lines
        run_with(&p, PAYLOAD).unwrap();
        let again = fs::read_to_string(&p.state).unwrap();
        assert_eq!(again, state);
    }

    #[test]
    fn wifi_payload_is_join_only() {
        let root = tmp("aginx-pair-apply-wifi");
        let p = paths(&root);
        fs::write(&p.state, "wifi fail no /etc/wifi.conf\ntime run\n").unwrap();
        let msg = run_with(&p, "WIFI:T:WPA;S:home;P:secret;;").unwrap();
        assert_eq!(msg, "网已连 home");
        assert!(p.wifi_conf.exists());
        assert!(!p.env_file.exists()); // no identity to merge
        let state = fs::read_to_string(&p.state).unwrap();
        assert!(state.contains("wifi ok home\n"));
        assert!(state.contains("dhcp ok 192.168.1.42\n"));
        assert!(state.contains("internet ok paired"));
        assert!(state.contains("time run\n")); // untouched — clock not our business here
    }

    #[test]
    fn join_failure_writes_nothing() {
        let root = tmp("aginx-pair-apply-fail");
        let mut p = paths(&root);
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p.net_join, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&p.net_join, "#!/bin/sh\nexit 1\n").unwrap();
        fs::write(&p.state, "wifi fail no /etc/wifi.conf\n").unwrap();
        let err = run_with(&p, PAYLOAD).unwrap_err();
        assert!(matches!(err, ApplyErr::Step(_)));
        assert!(!p.wifi_conf.exists());
        assert!(!p.env_file.exists());
        assert_eq!(fs::read_to_string(&p.state).unwrap(), "wifi fail no /etc/wifi.conf\n");
    }

    #[test]
    fn no_lease_runs_udhcpc_then_still_fails_closed() {
        // 2026-09-09 蛋首配回归：net-join 装完钥匙但 iface 无地址时，必须
        // 走 udhcpc 租约腿；租约不来仍不落 wifi.conf。
        let root = tmp("aginx-pair-apply-dhcp");
        let mut p = paths(&root);
        // ip 永不报地址 → 租约腿必走；udhcpc 打标证自己被叫过
        fs::write(&p.ip, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&p.udhcpc, format!("#!/bin/sh\ntouch {}\nexit 0\n", root.join("udhcpc.ran").display())).unwrap();
        fs::write(&p.state, "wifi fail no /etc/wifi.conf\n").unwrap();
        match run_with(&p, "WIFI:T:WPA;S:home;P:secret;;") {
            Err(ApplyErr::Step(e)) => assert!(e.contains("没拿到地址"), "{e}"),
            other => panic!("expected Step(没拿到地址), got {other:?}"),
        }
        assert!(root.join("udhcpc.ran").exists(), "udhcpc leg never ran");
        assert!(!p.wifi_conf.exists(), "no lease must not persist");
    }

    #[test]
    fn bad_payload_is_usage_error() {
        let root = tmp("aginx-pair-apply-bad");
        let p = paths(&root);
        assert_eq!(run_with(&p, "hello world"), Err(ApplyErr::BadPayload));
        // half identity: empty segment must not half-apply
        assert_eq!(run_with(&p, "AGINXPAIR1|ssid||key|gw|sec"), Err(ApplyErr::BadPayload));
        assert!(!p.wifi_conf.exists());
    }
}
