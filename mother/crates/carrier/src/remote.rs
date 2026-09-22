//! 远程化身句柄（CARRIER.md §3.3 远程类形态）。
//!
//! 远程化身 = 别人网关上的分身。注册成本地句柄后，`agent list` 同构可见、
//! `acp`/ask 可对话——实际经 aginx 网关 agent:// 外部协议转发（initialize
//! + prompt，访客 token 鉴权），本机化身和远程化身对使用者同构。
//!
//! 真源 `~/.aginx/carrier/remote-agents.json`；会话连续性靠网关侧收割的
//! sessionId 回喂（本文件不存任何会话状态）。token 是网关层访客凭证，
//! 文件按 0600 落盘。

use std::path::PathBuf;

use carrier_gateway::agent_client::AgentEndpoint;

use crate::RemoteAction;

/// 一个远程化身句柄：本地别名 → 目标网关 + 网关上的分身名。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RemoteHandle {
    /// 本地别名（对使用者即化身名；同 clone 名规则）
    pub name: String,
    /// 规范化网关地址（host 形态，无 path 后缀）
    pub url: String,
    /// 目标网关上的分身名（URL path 缺省时 = 本地别名）
    pub agent: String,
    #[serde(default)]
    pub display_name: String,
    /// 网关层访客 token（私有网关准入口；public 网关可空）
    #[serde(default)]
    pub token: Option<String>,
}

/// 注册表文件形状（versioned，向后迁移留钩子）。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct RegistryFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    handles: Vec<RemoteHandle>,
}

fn default_version() -> u32 {
    1
}

/// 注册表路径：`~/.aginx/carrier/remote-agents.json`。
pub fn path() -> PathBuf {
    carrier_types::config::home_dir().join("remote-agents.json")
}

/// 读注册表。文件不存在 = 空集；解析失败 = Err（add/remove 拒绝在损坏
/// 文件上继续写，防静默清空）。
fn load_from(path: &std::path::Path) -> Result<Vec<RemoteHandle>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取 {} 失败: {e}", path.display())),
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: RegistryFile =
        serde_json::from_str(&raw).map_err(|e| format!("{} 不是合法注册表 JSON: {e}", path.display()))?;
    Ok(file.handles)
}

/// 宽松读（acp/list 用）：损坏时 stderr 告警 + 空集，绝不挡对话路径。
pub fn load() -> Vec<RemoteHandle> {
    load_from(&path()).unwrap_or_else(|e| {
        eprintln!("警告：远程化身注册表读取失败（按无远程化身处理）���—{e}");
        Vec::new()
    })
}

/// 按别名查句柄。
pub fn find(name: &str) -> Option<RemoteHandle> {
    load().into_iter().find(|h| h.name == name)
}

fn save_to(path: &std::path::Path, handles: &[RemoteHandle]) -> Result<(), String> {
    let file = RegistryFile {
        version: 1,
        handles: handles.to_vec(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(path, body).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    // token 落在此文件——与 .env 同级的机密，0600 收权。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn save(handles: &[RemoteHandle]) -> Result<(), String> {
    save_to(&path(), handles)
}

/// 拆 `agent://<host>[/分身名]` → （规范化 host 地址, 可选目标分身）。
/// host 形态校验复用 `AgentEndpoint::parse_url`（.relay. 结构 + 端口）。
pub fn split_url(url: &str) -> Result<(String, Option<String>), String> {
    let rest = url
        .strip_prefix("agent://")
        .ok_or("地址需 agent:// 形态：agent://<id>.relay.<domain>[:port][/分身名]")?;
    let mut it = rest.splitn(2, '/');
    let host = it.next().unwrap_or("");
    let tail = it.next().unwrap_or("").trim_matches('/');
    if host.is_empty() || AgentEndpoint::parse_url(&format!("agent://{host}")).is_none() {
        return Err(format!(
            "无法解析 {url}：host 段需 <id>.relay.<domain> 形态（可带 :port）"
        ));
    }
    if !tail.is_empty() && !carrier_clone::market::valid_clone_name(tail) {
        return Err(format!("地址里的分身名 {tail:?} 不合法（小写字母/数字/连字符）"));
    }
    let agent = if tail.is_empty() { None } else { Some(tail.to_string()) };
    Ok((format!("agent://{host}"), agent))
}

/// 句柄 → 网关端点：解析 + 本机 relay secret 注入（同一张 relay 网网级
/// 凭证共享）+ 访客 token（仅填空，不覆盖端点已有值）。
pub fn endpoint(handle: &RemoteHandle) -> Option<AgentEndpoint> {
    let mut ep = AgentEndpoint::from_url_with_local_secret(&handle.url)?;
    if ep.auth_token.is_none() {
        ep.auth_token = handle
            .token
            .clone()
            .filter(|t| !t.trim().is_empty());
    }
    Some(ep)
}

/// `agent remote` 子命令：纯注册表文件操作，不 boot kernel（装卸竞态
/// 教训 + 无需 brain）。
pub fn run_remote(action: RemoteAction) -> anyhow::Result<()> {
    match action {
        RemoteAction::Add {
            name,
            url,
            display_name,
            token,
        } => add(name, url, display_name, token),
        RemoteAction::Remove { name } => remove(&name),
    }
}

fn add(
    name: String,
    url: String,
    display_name: Option<String>,
    token: Option<String>,
) -> anyhow::Result<()> {
    if !carrier_clone::market::valid_clone_name(&name) {
        anyhow::bail!("化身名只允许小写字母、数字与连字符（1-64 位）");
    }
    // 同名互斥两个方向都在注册期拦死（install 侧反向拦）：acp 的本地/远程
    // 分派依赖名字不撞。
    if carrier_kernel::aginx_net::registration_exists_default(&name) {
        anyhow::bail!("本机已有同名化身 {name}——远程句柄请换一个别名");
    }
    let (url_norm, agent_from_path) = split_url(&url).map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut handles = load_from(&path())
        .map_err(|e| anyhow::anyhow!("注册表损坏，拒绝写入（先人工检查 {}）：{e}", path().display()))?;
    if handles.iter().any(|h| h.name == name) {
        anyhow::bail!("已有同名远程化身 {name}——先 `agent remote remove {name}`");
    }
    // 名字后面要 move 进句柄，留一份给输出。
    let alias = name.clone();

    let handle = RemoteHandle {
        agent: agent_from_path.unwrap_or_else(|| name.clone()),
        display_name: display_name
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| name.clone()),
        token: token.filter(|t| !t.trim().is_empty()),
        name,
        url: url_norm,
    };
    let target = format!("{}/{}", handle.url, handle.agent);
    handles.push(handle);
    save(&handles).map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("已注册远程化身（{alias} → {target}）");
    println!("对话：aginx-carrier acp --clone {alias}（aterm 列表同构可见）");
    Ok(())
}

fn remove(name: &str) -> anyhow::Result<()> {
    let mut handles = load_from(&path())
        .map_err(|e| anyhow::anyhow!("注册表损坏，拒绝写入（先人工检查 {}）：{e}", path().display()))?;
    let before = handles.len();
    handles.retain(|h| h.name != name);
    if handles.len() == before {
        anyhow::bail!("没有叫 {name} 的远程化身句柄");
    }
    save(&handles).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("已移除远程化身句柄 {name}（不影响对方网关）");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("remote-agents-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn split_url_variants() {
        // host-only：无 path
        let (url, agent) = split_url("agent://sv1.relay.aginx.net").unwrap();
        assert_eq!(url, "agent://sv1.relay.aginx.net");
        assert_eq!(agent, None);

        // host + path：path 是目标分身名，URL 规范化剥掉
        let (url, agent) = split_url("agent://sv1.relay.aginx.net/kefu").unwrap();
        assert_eq!(url, "agent://sv1.relay.aginx.net");
        assert_eq!(agent.as_deref(), Some("kefu"));

        // 端口 + path
        let (url, agent) = split_url("agent://abc.relay.example.com:9443/claude").unwrap();
        assert_eq!(url, "agent://abc.relay.example.com:9443");
        assert_eq!(agent.as_deref(), Some("claude"));

        // 尾部斜杠剥掉
        let (_, agent) = split_url("agent://a.relay.b.io/x/").unwrap();
        assert_eq!(agent.as_deref(), Some("x"));
    }

    #[test]
    fn split_url_rejects_garbage() {
        assert!(split_url("https://relay.aginx.net").is_err());
        assert!(split_url("agent://192.168.1.1:86").is_err()); // 无 .relay. 结构
        assert!(split_url("agent://only.relay.").is_err());
        assert!(split_url("agent://sv1.relay.aginx.net/Bad_Name").is_err()); // 分身名字符集
    }

    #[test]
    fn registry_roundtrip_and_missing_file() {
        let p = scratch();
        // 不存在 = 空集
        assert!(load_from(&p).unwrap().is_empty());

        let handles = vec![RemoteHandle {
            name: "kefu".into(),
            url: "agent://sv1.relay.aginx.net".into(),
            agent: "kefu".into(),
            display_name: "老王的客服".into(),
            token: Some("tok-1".into()),
        }];
        save_to(&p, &handles).unwrap();

        let back = load_from(&p).unwrap();
        assert_eq!(back, handles);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn corrupt_registry_rejected_not_emptied() {
        let p = scratch();
        std::fs::write(&p, "{ not json").unwrap();
        assert!(load_from(&p).is_err(), "损坏文件必须报错，不能静默当空集（防覆盖）");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn endpoint_fills_token_only_when_empty() {
        let mut h = RemoteHandle {
            name: "n".into(),
            url: "agent://sv1.relay.aginx.net:9443".into(),
            agent: "a".into(),
            display_name: String::new(),
            token: Some("tok".into()),
        };
        let ep = endpoint(&h).unwrap();
        assert_eq!(ep.target, "sv1");
        assert_eq!(ep.port, 9443);
        assert_eq!(ep.auth_token.as_deref(), Some("tok"));

        // token 空串/None → 端点保持无鉴权（public 网关）
        h.token = Some("  ".into());
        assert!(endpoint(&h).unwrap().auth_token.is_none());
    }
}
