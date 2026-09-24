//! 网关 agent 的添加台账 + 联系人记忆（webui 第三刀，批2 语义；
//! 第五刀扩远程网关地址簿）。
//!
//! `~/.aginx/carrier/webui/external-tools.json`：added 清单（哪些网关
//! agent 进了联系人）+ per (agent, sender) 的联系人记忆（claude 真
//! session_id 与消息流水）+ 远程网关地址簿 + 远程联系人展示元数据。
//! 会话列表不再本地记——历史会话对话框透传网关台账 sessions/list
//!（§2.4.1）；点选某条历史 → set_active_session 把续接 id 记到联系人，
//! 下轮 prompt 回喂。原子写：tmp + rename；损坏重置为空（台账丢失只
//! 是重新添加，不致命）。
//!
//! 联系人 id 语法：裸 id = 本机网关 agent；`@<target>~<agent>` = 远程
//! 网关（缺口3 统一 aginx 流程——用别人家分身走标准 agent:// 路径）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMessage {
    role: String,
    text: String,
    #[serde(default)]
    ts: String,
}

/// 一个联系人的本地记忆：当前续接会话 + 消息流水。
/// 换会话（切历史/新建）只切 session_id，流水线性追加（微信式）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredContact {
    /// agent 侧会话 id（网关收割），下轮回喂续接；None = 下轮开新会话
    session_id: Option<String>,
    messages: Vec<StoredMessage>,
}

/// 一个凭证槽。token + 名字（owner：配对设备名；visitor：同意流 client 名）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredential {
    token: String,
    name: String,
}

/// 远程网关地址簿条目（第五刀；第六刀加绑定凭证；第九刀双槽化——
/// 主人绑定与访客授权各自独立，同一台 webui 对同一网关可同时是
/// 主人（配对码绑定）和访客（被授权），凭证互不覆写。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredGateway {
    /// relay 路由 id（agent://<target>.relay.<domain> 的 target）
    target: String,
    /// 原始 URL（保留 domain/port，重连时重新解析）
    url: String,
    /// 主人槽：bindDevice 配对所得 Bound token（管理法 + 全量 agent）。
    #[serde(default)]
    owner: Option<StoredCredential>,
    /// 访客槽：同意流发放的 Authorized token（allowedAgents 范围内）。
    #[serde(default)]
    visitor: Option<StoredCredential>,
    // ── 单槽时代遗留字段（load 时折叠进上面对应槽位，之后恒 None） ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
    /// 旧语义："visitor" = 单槽凭证来自同意流
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

impl StoredGateway {
    /// 单槽 → 双槽迁移（幂等：槽位已有凭证则丢弃遗留字段）。
    fn migrate(&mut self) {
        if let Some(token) = self.token.take() {
            let name = self.device_name.take().unwrap_or_default();
            let cred = StoredCredential {
                token,
                name: if name.is_empty() { "webui".into() } else { name },
            };
            if self.role.as_deref() == Some("visitor") && self.visitor.is_none() {
                self.visitor = Some(cred);
            } else if self.owner.is_none() {
                self.owner = Some(cred);
            }
        }
        self.device_name = None;
        self.role = None;
    }
}

/// 远程联系人的展示元数据——本机不连远程也能渲染联系人列表
///（网关离线时联系人不消失，聊天时才报错）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAgentMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub agent_type: String,
    /// 所属远程网关 target
    pub gateway: String,
}

/// 地址簿网关条目（对外形状：token 不出 store，只报绑定状态）。
#[derive(Debug, Clone)]
pub struct GatewayEntry {
    pub target: String,
    pub url: String,
    /// 任一槽位有凭证（主人或访客）
    pub bound: bool,
    /// 展示名（主人=设备名；访客-only=client 名）
    pub device_name: Option<String>,
    /// 主展示角色："owner"（主人槽在）/ "visitor"（仅访客槽）
    pub role: String,
    /// 主人槽（bindDevice）有凭证——👥 面板可用
    pub owner_bound: bool,
    /// 访客槽（同意流）有凭证
    pub visitor_authorized: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreData {
    #[serde(default)]
    added: Vec<String>,
    /// key = "{agent}/{sender}"
    #[serde(default)]
    contacts: HashMap<String, StoredContact>,
    #[serde(default)]
    gateways: Vec<StoredGateway>,
    /// key = 复合联系人 id（@target~agent）
    #[serde(default)]
    remote_meta: HashMap<String, StoredAgentMeta>,
}

pub struct ToolStore {
    path: PathBuf,
    data: Mutex<StoreData>,
}

impl ToolStore {
    /// 载入或新建。损坏时重置为空并 warn（不阻断 webui 启动）。
    /// 单槽时代的遗留字段在此折叠进双槽（幂等）。
    pub fn load(path: PathBuf) -> Self {
        let mut data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| {
                if path.exists() {
                    tracing::warn!(path = %path.display(), "external-tools.json 损坏，已重置");
                }
                StoreData::default()
            });
        for g in &mut data.gateways {
            g.migrate();
        }
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    fn persist(&self, data: &StoreData) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        match serde_json::to_string_pretty(data) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, &self.path)) {
                    tracing::warn!(error = %e, "external-tools.json 写盘失败");
                }
            }
            Err(e) => tracing::warn!(error = %e, "external-tools.json 序列化失败"),
        }
    }

    pub fn added(&self) -> Vec<String> {
        self.data.lock().unwrap().added.clone()
    }

    pub fn is_added(&self, id: &str) -> bool {
        self.data.lock().unwrap().added.iter().any(|a| a == id)
    }

    pub fn add(&self, id: &str) {
        let mut data = self.data.lock().unwrap();
        if !data.added.iter().any(|a| a == id) {
            data.added.push(id.to_string());
            self.persist(&data);
        }
    }

    /// 移出台账并清掉该工具全部联系人记忆。
    pub fn remove(&self, id: &str) {
        let mut data = self.data.lock().unwrap();
        data.added.retain(|a| a != id);
        let prefix = format!("{id}/");
        data.contacts.retain(|k, _| !k.starts_with(&prefix));
        data.remote_meta.remove(id);
        self.persist(&data);
    }

    // ── 远程网关地址簿（第五刀；第六刀加绑定凭证） ──

    pub fn gateways(&self) -> Vec<GatewayEntry> {
        self.data
            .lock()
            .unwrap()
            .gateways
            .iter()
            .map(|g| {
                let owner_bound = g.owner.is_some();
                let visitor_authorized = g.visitor.is_some();
                GatewayEntry {
                    target: g.target.clone(),
                    url: g.url.clone(),
                    bound: owner_bound || visitor_authorized,
                    device_name: g
                        .owner
                        .as_ref()
                        .map(|c| c.name.clone())
                        .or_else(|| g.visitor.as_ref().map(|c| c.name.clone())),
                    role: if owner_bound { "owner" } else { "visitor" }.to_string(),
                    owner_bound,
                    visitor_authorized,
                }
            })
            .collect()
    }

    pub fn gateway_url(&self, target: &str) -> Option<String> {
        self.data
            .lock()
            .unwrap()
            .gateways
            .iter()
            .find(|g| g.target == target)
            .map(|g| g.url.clone())
    }

    /// 会话用 token（端点注入用）。主人槽优先（全量 agent + 管理法），
    /// 无主人槽回落访客槽（allowedAgents 范围内）。None = 未绑定/公开网关。
    pub fn gateway_token(&self, target: &str) -> Option<String> {
        self.data
            .lock()
            .unwrap()
            .gateways
            .iter()
            .find(|g| g.target == target)
            .and_then(|g| {
                g.owner
                    .as_ref()
                    .map(|c| c.token.clone())
                    .or_else(|| g.visitor.as_ref().map(|c| c.token.clone()))
            })
    }

    /// 主人 token（👥 面板/管理法专用——Bound 连接，访客 token 打不开）。
    pub fn gateway_owner_token(&self, target: &str) -> Option<String> {
        self.data
            .lock()
            .unwrap()
            .gateways
            .iter()
            .find(|g| g.target == target)
            .and_then(|g| g.owner.as_ref().map(|c| c.token.clone()))
    }

    /// 收录远程网关（幂等，URL 以最新为准；不触碰绑定凭证）。
    pub fn add_gateway(&self, target: &str, url: &str) {
        let mut data = self.data.lock().unwrap();
        match data.gateways.iter_mut().find(|g| g.target == target) {
            Some(g) => g.url = url.to_string(),
            None => data.gateways.push(StoredGateway {
                target: target.to_string(),
                url: url.to_string(),
                owner: None,
                visitor: None,
                token: None,
                device_name: None,
                role: None,
            }),
        }
        self.persist(&data);
    }

    /// 记录配对结果（bindDevice 成功后调用）——只写主人槽，
    /// 访客槽里的授权凭证不受影响（双槽不互踢）。
    pub fn set_gateway_binding(&self, target: &str, token: &str, device_name: &str) {
        let mut data = self.data.lock().unwrap();
        if let Some(g) = data.gateways.iter_mut().find(|g| g.target == target) {
            g.owner = Some(StoredCredential {
                token: token.to_string(),
                name: device_name.to_string(),
            });
            self.persist(&data);
        }
    }

    /// 记录同意流授权（checkAccess 取票成功后调用）——只写访客槽，
    /// 主人绑定凭证不受影响（双槽不互踢）。
    pub fn set_gateway_authorized(&self, target: &str, token: &str, client_name: &str) {
        let mut data = self.data.lock().unwrap();
        if let Some(g) = data.gateways.iter_mut().find(|g| g.target == target) {
            g.visitor = Some(StoredCredential {
                token: token.to_string(),
                name: client_name.to_string(),
            });
            self.persist(&data);
        }
    }

    /// 移除远程网关，级联清掉它名下全部远程联系人（added/记忆/元数据）。
    pub fn remove_gateway(&self, target: &str) {
        let mut data = self.data.lock().unwrap();
        data.gateways.retain(|g| g.target != target);
        let prefix = format!("@{target}~");
        data.added.retain(|a| !a.starts_with(&prefix));
        data.contacts.retain(|k, _| !k.starts_with(&prefix));
        data.remote_meta.retain(|k, _| !k.starts_with(&prefix));
        self.persist(&data);
    }

    // ── 远程联系人元数据 ──

    pub fn set_remote_meta(&self, id: &str, meta: StoredAgentMeta) {
        let mut data = self.data.lock().unwrap();
        data.remote_meta.insert(id.to_string(), meta);
        self.persist(&data);
    }

    /// 已添加的远程联系人（复合 id + 展示元数据），零网络即可渲染。
    pub fn added_remote(&self) -> Vec<(String, StoredAgentMeta)> {
        let data = self.data.lock().unwrap();
        data.added
            .iter()
            .filter(|a| a.starts_with('@'))
            .filter_map(|a| data.remote_meta.get(a).map(|m| (a.clone(), m.clone())))
            .collect()
    }

    fn contact_key(agent: &str, sender: &str) -> String {
        format!("{agent}/{sender}")
    }

    /// 取联系人记忆。返回 (当前续接 session_id, 消息流水)。
    pub fn session(&self, agent: &str, sender: &str) -> (Option<String>, Vec<(String, String)>) {
        let data = self.data.lock().unwrap();
        data.contacts
            .get(&Self::contact_key(agent, sender))
            .map(|c| {
                (
                    c.session_id.clone(),
                    c.messages.iter().map(|m| (m.role.clone(), m.text.clone())).collect(),
                )
            })
            .unwrap_or((None, Vec::new()))
    }

    /// 切续接会话（历史对话框点选 / 新建）。None = 下轮开新会话。
    pub fn set_active_session(&self, agent: &str, sender: &str, sid: Option<&str>) {
        let mut data = self.data.lock().unwrap();
        let entry = data
            .contacts
            .entry(Self::contact_key(agent, sender))
            .or_default();
        entry.session_id = sid.map(String::from);
        self.persist(&data);
    }

    /// 追加一轮消息（user + assistant）并更新续接 session_id（收割到才更新）。
    pub fn append_turn(
        &self,
        agent: &str,
        sender: &str,
        user_text: &str,
        assistant_text: &str,
        session_id: Option<&str>,
    ) {
        let mut data = self.data.lock().unwrap();
        let entry = data
            .contacts
            .entry(Self::contact_key(agent, sender))
            .or_default();
        let ts = chrono_like_now();
        entry.messages.push(StoredMessage {
            role: "user".into(),
            text: user_text.to_string(),
            ts: ts.clone(),
        });
        entry.messages.push(StoredMessage {
            role: "assistant".into(),
            text: assistant_text.to_string(),
            ts,
        });
        if let Some(sid) = session_id {
            entry.session_id = Some(sid.to_string());
        }
        self.persist(&data);
    }
}

/// RFC3339 UTC 时间戳（毫秒精度——同秒消息按插入序展示）。
fn chrono_like_now() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (secs, ms) = (dur.as_secs(), dur.subsec_millis());
    // 1970-01-01T00:00:00.000Z + secs → 手写换算避免引入 chrono 依赖
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days（Howard Hinnant 算法）
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path) -> ToolStore {
        ToolStore::load(dir.join("external-tools.json"))
    }

    #[test]
    fn round_trip_add_append_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        assert!(!s.is_added("claude"));

        s.add("claude");
        assert!(s.is_added("claude"));
        s.add("claude"); // 幂等
        assert_eq!(s.added(), vec!["claude".to_string()]);

        s.append_turn("claude", "web:u1", "你好", "收到", Some("sid-1"));
        let (sid, msgs) = s.session("claude", "web:u1");
        assert_eq!(sid.as_deref(), Some("sid-1"));
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], ("user".into(), "你好".into()));
        assert_eq!(msgs[1], ("assistant".into(), "收到".into()));

        // 另一 sender 隔离
        let (sid2, msgs2) = s.session("claude", "web:u2");
        assert_eq!(sid2, None);
        assert!(msgs2.is_empty());

        // remove 清台账 + 联系人
        s.remove("claude");
        assert!(!s.is_added("claude"));
        let (sid3, _) = s.session("claude", "web:u1");
        assert_eq!(sid3, None);

        // 重新 load（持久化验证）
        let s2 = store(tmp.path());
        assert!(!s2.is_added("claude"));
    }

    #[test]
    fn set_active_session_switches_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.add("quanyi");
        s.append_turn("quanyi", "web:u1", "第一句", "收到", Some("sid-a"));
        assert_eq!(s.session("quanyi", "web:u1").0.as_deref(), Some("sid-a"));

        // 历史对话框点选旧会话
        s.set_active_session("quanyi", "web:u1", Some("sid-old"));
        assert_eq!(s.session("quanyi", "web:u1").0.as_deref(), Some("sid-old"));

        // 新建会话
        s.set_active_session("quanyi", "web:u1", None);
        assert_eq!(s.session("quanyi", "web:u1").0, None);

        // 未收割到 id 的轮（raw 方言）不动续接 id
        s.set_active_session("quanyi", "web:u1", Some("sid-b"));
        s.append_turn("quanyi", "web:u1", "再问", "答", None);
        assert_eq!(s.session("quanyi", "web:u1").0.as_deref(), Some("sid-b"));
    }

    #[test]
    fn persist_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let s = store(tmp.path());
            s.add("copilot");
            s.append_turn("copilot", "web:u2", "q", "a", Some("sid-2"));
        }
        let s2 = store(tmp.path());
        assert!(s2.is_added("copilot"));
        let (sid, msgs) = s2.session("copilot", "web:u2");
        assert_eq!(sid.as_deref(), Some("sid-2"));
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn corrupt_file_resets() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("external-tools.json");
        std::fs::write(&path, "{broken json").unwrap();
        let s = ToolStore::load(path);
        assert!(!s.is_added("claude"));
        // 仍可正常写
        s.add("claude");
        assert!(s.is_added("claude"));
    }

    #[test]
    fn timestamp_shape() {
        let ts = chrono_like_now();
        // 2026-08-24T..:..:...###Z（毫秒精度）
        assert_eq!(ts.len(), 24);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[19..20], ".");
    }

    // ── 远程网关地址簿（第五刀） ──

    #[test]
    fn gateway_book_round_trip_and_cascade() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let s = store(tmp.path());
            s.add_gateway("selvkwjv", "agent://selvkwjv.relay.aginx.net");
            s.add_gateway("selvkwjv", "agent://selvkwjv.relay.aginx.net:8443"); // 更新 URL
            s.add_gateway("other", "agent://other.relay.aginx.net");
            assert_eq!(s.gateways().len(), 2);
            assert_eq!(
                s.gateway_url("selvkwjv").as_deref(),
                Some("agent://selvkwjv.relay.aginx.net:8443")
            );

            // 远程联系人：复合 id + 元数据 + 消息
            let cid = "@selvkwjv~clone-creator";
            s.add(cid);
            s.set_remote_meta(
                cid,
                StoredAgentMeta {
                    name: "分身创造者".into(),
                    description: "造分身".into(),
                    agent_type: "aginx-carrier".into(),
                    gateway: "selvkwjv".into(),
                },
            );
            s.append_turn(cid, "web:u1", "你好", "收到", Some("sid-r1"));
            assert_eq!(s.added_remote().len(), 1);
            assert_eq!(s.added_remote()[0].1.gateway, "selvkwjv");

            // 移网关：级联清联系人，别的网关不受影响
            s.add("@other~agent1");
            s.remove_gateway("selvkwjv");
            assert!(s.gateway_url("selvkwjv").is_none());
            assert!(!s.is_added(cid));
            assert!(s.added_remote().is_empty());
            let (sid, msgs) = s.session(cid, "web:u1");
            assert_eq!(sid, None);
            assert!(msgs.is_empty());
            assert!(s.is_added("@other~agent1"));
        }
        // 持久化验证：重载后 other 网关和它的联系人还在
        let s2 = store(tmp.path());
        assert_eq!(s2.gateways().len(), 1);
        assert_eq!(s2.gateways()[0].target, "other");
        assert!(s2.is_added("@other~agent1"));
    }

    #[test]
    fn gateway_binding_round_trip_and_compat() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("external-tools.json");
        // 第五刀时代的旧文件（无 token 字段）必须平滑升级
        std::fs::write(
            &path,
            r#"{"added":[],"contacts":{},"gateways":[{"target":"sv1","url":"agent://sv1.relay.aginx.net"}],"remote_meta":{}}"#,
        )
        .unwrap();
        let s = ToolStore::load(path);
        assert_eq!(s.gateways().len(), 1);
        assert!(!s.gateways()[0].bound, "旧文件无凭证 → 未绑定");
        assert_eq!(s.gateway_token("sv1"), None);

        // 配对成功 → 记凭证；add_gateway 更新 URL 不碰凭证
        s.set_gateway_binding("sv1", "token-abc123", "webui");
        assert_eq!(s.gateway_token("sv1").as_deref(), Some("token-abc123"));
        assert!(s.gateways()[0].bound);
        assert_eq!(s.gateways()[0].device_name.as_deref(), Some("webui"));
        s.add_gateway("sv1", "agent://sv1.relay.aginx.net:9443");
        assert_eq!(s.gateway_token("sv1").as_deref(), Some("token-abc123"));

        // 重载后凭证还在（token 只落本机 store，���出网关侧以外的地方）
        let s2 = ToolStore::load(tmp.path().join("external-tools.json"));
        assert_eq!(s2.gateway_token("sv1").as_deref(), Some("token-abc123"));
        assert!(s2.gateways()[0].bound);

        // 移网关 = 凭证一并清
        s2.remove_gateway("sv1");
        assert_eq!(s2.gateway_token("sv1"), None);
    }

    #[test]
    fn gateway_dual_slot_no_clobber() {
        // 第九刀：主人绑定与访客授权双槽——后到的凭证不再覆写先来的。
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.add_gateway("sv2", "agent://sv2.relay.aginx.net");

        // 主人配对 → 主人槽
        s.set_gateway_binding("sv2", "bound-token-1", "webui");
        assert_eq!(s.gateway_owner_token("sv2").as_deref(), Some("bound-token-1"));
        assert_eq!(s.gateway_token("sv2").as_deref(), Some("bound-token-1"));

        // 同意流取票 → 访客槽；主人槽原样（旧单槽实现在这里覆写——回归点）
        s.set_gateway_authorized("sv2", "ac-visitor-9", "小明");
        assert_eq!(s.gateway_owner_token("sv2").as_deref(), Some("bound-token-1"), "访客授权不得覆写主人绑定");
        let g = &s.gateways()[0];
        assert!(g.owner_bound && g.visitor_authorized, "双身份并存");
        assert_eq!(g.role, "owner", "会话/主展示优先主人身份");
        assert_eq!(s.gateway_token("sv2").as_deref(), Some("bound-token-1"), "会话 token 主人优先");

        // 重载后双槽持久
        let s2 = store(tmp.path());
        let g2 = &s2.gateways()[0];
        assert!(g2.owner_bound && g2.visitor_authorized);
        assert_eq!(s2.gateway_owner_token("sv2").as_deref(), Some("bound-token-1"));
        assert_eq!(g2.device_name.as_deref(), Some("webui"), "展示名 = 主人设备名");

        // 移网关 = 两把凭证一并清
        s2.remove_gateway("sv2");
        assert_eq!(s2.gateway_token("sv2"), None);
    }

    #[test]
    fn gateway_visitor_only_falls_back() {
        // 仅访客授权（没配对过）：会话用访客 token，主人面板无凭证。
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.add_gateway("sv3", "agent://sv3.relay.aginx.net");
        s.set_gateway_authorized("sv3", "ac-only", "访客甲");
        assert_eq!(s.gateway_owner_token("sv3"), None);
        assert_eq!(s.gateway_token("sv3").as_deref(), Some("ac-only"));
        let g = &s.gateways()[0];
        assert!(g.bound && !g.owner_bound && g.visitor_authorized);
        assert_eq!(g.role, "visitor");
        assert_eq!(g.device_name.as_deref(), Some("访客甲"));
    }

    #[test]
    fn gateway_legacy_single_slot_migrates() {
        // 第八刀单槽文件：role=visitor 的凭证迁进访客槽；无 role 的迁主人槽。
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("external-tools.json");
        std::fs::write(
            &path,
            r#"{"gateways":[
                {"target":"old-vis","url":"agent://old.relay.aginx.net","token":"ac-legacy","device_name":"小明","role":"visitor"},
                {"target":"old-own","url":"agent://old.relay.aginx.net","token":"bound-legacy","device_name":"webui"}
            ]}"#,
        )
        .unwrap();
        let s = ToolStore::load(path);
        let gv = s.gateways().into_iter().find(|g| g.target == "old-vis").unwrap();
        assert!(gv.visitor_authorized && !gv.owner_bound, "role=visitor 的单槽凭证进访客槽");
        assert_eq!(s.gateway_token("old-vis").as_deref(), Some("ac-legacy"));
        let go = s.gateways().into_iter().find(|g| g.target == "old-own").unwrap();
        assert!(go.owner_bound && !go.visitor_authorized, "无 role 的单槽凭证进主人槽");
        assert_eq!(s.gateway_owner_token("old-own").as_deref(), Some("bound-legacy"));
    }
}
