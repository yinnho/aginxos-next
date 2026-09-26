// host — 母体的运行时宿主（刀2：server 直调 carrier kernel）。
//
// 宪法：这个系统就是智能体，必须以最直接的运行方式跑——OS 进程=agent
// 进程，盒内零 hop。母体 server 不再 spawn aginx-runtime 子进程，而是
// 进程内 boot kernel、轮到时直调 send_message_with_handle；工具帧经
// TurnObserver（刀2-2a）回流落账。aginx-carrier 至此并入系统，不再
// 单独存在。
//
// D8 帧账（agi jsonl）：
//   me 的账     home/sessions/main.jsonl（home 根，FS.md：母体不是文件夹）
//   助理的账    home/workflows/<名>/sessions/main.jsonl
// 边界帧（request/done）由 run_turn 记——它知道回合号；循环内的
// tool_call/tool_result/steer 由 LedgerObserver 记。
//
// brain.json 桥：kernel boot 只认 home/brain.json（或 Hub 拉取）。
// 设备上刀3 烤树之前，env（AGINX_BRAIN_URL/AGINXBRAIN_API_KEY）仍要能
// 点火——缺席且 env 在场时一次性合成最小 brain.json，此后文件是真源。

use crate::front::{FrontDesk, SteerOutcome, MOTHER};
use agi::{Done, Frame};
use carrier_kernel::kernel::CarrierKernel;
use carrier_types::agent::AgentManifest;
use carrier_types::config::{KernelConfig, SYSTEM_AGENT_ME};
use carrier_types::observer::TurnObserver;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 工具输出进账前的字符上限（承 turn.rs 旧值）：CLI 倾倒可以肥，账本
/// 不能无边；引擎侧另有 brain 截断，这里的 20 万字符是审计留量。
const TOOL_OUT_MAX_CHARS: usize = 200_000;

/// /home 出厂树随镜像烤进（结构刀④）：真源=仓里 home/ 整树，烤线整树
/// 拷（docs/FS.md）；server 不种、不碰——boot 对家根只保 sessions/ 目录
/// 在位。OTA 永不覆盖 /home；重刷=新盘自带来树。
fn ensure_home_skeleton(home: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(home.join("sessions")) {
        eprintln!("mother: ensure home sessions dir failed: {e}");
    }
}

fn cap(s: &str) -> String {
    if s.chars().count() <= TOOL_OUT_MAX_CHARS {
        s.to_string()
    } else {
        let kept: String = s.chars().take(TOOL_OUT_MAX_CHARS).collect();
        format!("{kept}\n…[输出超限已截断]")
    }
}

/// 助理（旧化身）账本路径：home/workflows/<名>/sessions/main.jsonl。
/// me 走 home/sessions/main.jsonl。
pub fn agent_log(home: &std::path::Path, agent: &str) -> PathBuf {
    if agent == MOTHER {
        home.join("sessions").join(format!("{}.jsonl", crate::front::SESSION_MAIN))
    } else {
        home.join("workflows").join(agent).join("sessions")
            .join(format!("{}.jsonl", crate::front::SESSION_MAIN))
    }
}

/// D8 帧账观察者：kernel 轮里的工具事件 → agi 帧 → 会话账。
///
/// `current` 由 run_turn 在调 kernel 前设置（化身名 + 回合号）——一次只
/// 有一个轮在跑（front 层 turn 锁），不匹配 current 的事件不属于这个
/// 账本，跳过（kernel 内部 summarizer 等自调工具不进用户会话账）。
struct LedgerObserver {
    home: PathBuf,
    desk: Arc<FrontDesk>,
    current: Mutex<Option<(String, u64)>>,
}

impl LedgerObserver {
    fn set_current(&self, agent: &str, turn: u64) {
        *self.current.lock().unwrap_or_else(|p| p.into_inner()) = Some((agent.to_string(), turn));
    }

    fn clear_current(&self) {
        *self.current.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// 只给当前轮记账；附带回合号（steer 回执要用）。
    fn in_current_turn(&self, agent: &str) -> Option<u64> {
        let cur = self.current.lock().unwrap_or_else(|p| p.into_inner());
        match cur.as_ref() {
            Some((a, turn)) if a == agent => Some(*turn),
            _ => None,
        }
    }
}

impl TurnObserver for LedgerObserver {
    fn on_tool_call(&self, agent: &str, id: &str, tool: &str, args: &serde_json::Value) {
        let Some(turn) = self.in_current_turn(agent) else { return };
        let _ = turn; // 回合号不入 tool 帧（账序即轮序，done 盖章）
        let f = Frame::ToolCall(agi::ToolCall {
            id: id.to_string(),
            tool: tool.to_string(),
            args: args.clone(),
        });
        if let Err(e) = crate::ledger::append(&agent_log(&self.home, agent), &f) {
            eprintln!("ledger: tool_call append failed: {e}");
        }
    }

    fn on_tool_result(&self, agent: &str, id: &str, ok: bool, content: &str) {
        if self.in_current_turn(agent).is_none() {
            return;
        }
        // ok → code 0/out=载荷；!ok → code 1/err=载荷（协议同旧 spawn 路）
        let f = Frame::ToolResult(agi::ToolResult {
            id: id.to_string(),
            ok,
            code: if ok { 0 } else { 1 },
            out: if ok { cap(content) } else { String::new() },
            err: if ok { String::new() } else { cap(content) },
        });
        if let Err(e) = crate::ledger::append(&agent_log(&self.home, agent), &f) {
            eprintln!("ledger: tool_result append failed: {e}");
        }
    }

    fn drain_steers(&self, agent: &str) -> Vec<String> {
        let Some(turn) = self.in_current_turn(agent) else { return Vec::new() };
        let mut texts = Vec::new();
        for req in self.desk.take_steers() {
            // 先记账再供货（D8 铁律）；回执 Delivered(turn)——发送方
            // steered:true 即回，不等轮跑完。
            let f = Frame::Steer(agi::Steer { text: req.text.clone() });
            if crate::ledger::append(&agent_log(&self.home, agent), &f).is_err() {
                let _ = req.reply.send(SteerOutcome::TurnEnded);
                continue;
            }
            let _ = req.reply.send(SteerOutcome::Delivered(turn));
            texts.push(req.text);
        }
        texts
    }
}

/// 母体宿主：kernel + 它的运行时 + 账本观察者的句柄（与 kernel 里
/// 装的是同一份 Arc——run_turn 用它对准当前轮）。channels = iLink 通道
/// 管理器（必须随 Mother 活着——Drop 会拆通道线程）。
pub struct Mother {
    pub kernel: Arc<CarrierKernel>,
    observer: Arc<LedgerObserver>,
    rt: tokio::runtime::Runtime,
    home: PathBuf,
    #[allow(dead_code)] // 持有即在线：Drop 拆通道，字段不读
    channels: Option<carrier_runtime::channel_manager::ChannelManager>,
}

impl Mother {
    /// boot kernel 一次：brain 桥 → KernelConfig → set_self_handle →
    /// 装账本观察者 → reconcile（me + workflows/ 全员在册）。
    pub fn boot(home: PathBuf, desk: Arc<FrontDesk>) -> Result<Mother, String> {
        ensure_home_skeleton(&home);
        bridge_brain_json(&home)?;
        let config = KernelConfig {
            home_dir: home.clone(),
            data_dir: home.join("data"),
            workflows_dir: Some(home.join("workflows")),
            ..KernelConfig::default()
        };
        // RT 先于 kernel：boot 期若起后台任务得有活跃上下文；此后轮到
        // 时 block_on 也由它驱动。必须 multi-thread——kernel 轮路径里有
        // block_in_place + Handle::block_on 的同步桥（sender 历史富集），
        // current-thread 会 panic。并发度仍由 front 层 turn 锁保证
        // （同时只有一个 block_on 在跑）。
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        let kernel = {
            let _ctx = rt.enter();
            CarrierKernel::boot_with_config(config).map_err(|e| format!("kernel boot failed: {e}"))?
        };
        let kernel = Arc::new(kernel);
        kernel.set_self_handle();
        let observer = Arc::new(LedgerObserver {
            home: home.clone(),
            desk,
            current: Mutex::new(None),
        });
        kernel.set_turn_observer(Arc::clone(&observer) as Arc<dyn TurnObserver>);
        let mut mother = Mother {
            kernel: Arc::clone(&kernel),
            observer,
            rt,
            home,
            channels: None,
        };
        mother.reconcile()?;
        // 定时是母体职能（显示线刀A）：cron tick 循环随母体起。老路只有
        // aginx-carrier 守护会经 start_background_agents 触发它——server
        // 直调形态此前装而不触发，定时任务全瘫。必须在 reconcile 之后：
        // 循环启动即跑一次 reconcile_chains、每 tick 还会清孤儿 cron
        // （registry 无此 agent → remove_job），agent 未在册就开闸会把
        // 好好的 job 当孤儿误删。15s tick 是 tokio::spawn 的后台任务，
        // 跑在 worker 上，与轮 future 不抢。
        {
            let _ctx = mother.rt.enter();
            mother.kernel.start_cron_loop();
        }
        // iLink（微信）入站通道：watcher + 微信工具 + 出站注入（2026-09-26
        // 上机线）。start 在 RT 上下文里跑——bridge 与 poll 线程落在这台
        // runtime 上；cm 换进 Mother 活到进程终（Drop 会拆通道）。
        {
            match crate::channels::boot_ilink(&kernel) {
                Ok(mut cm) => {
                    let _ctx = mother.rt.enter();
                    mother.rt.block_on(cm.start());
                    eprintln!("mother: iLink channel online (weixin watcher + tools)");
                    mother.channels.replace(cm);
                }
                // 通道层是特性不是脊柱：起不来就少个微信面，母体（轮、
                // 定时、网关、ssh）照活——同一进程里 `?` 会连晨报一起殉。
                Err(e) => eprintln!("mother: iLink channel OFF ({e}) — mother continues"),
            }
        }
        Ok(mother)
    }

    /// 开机对账：me（workspace=home 根）+ workflows/ 每个目录一名助理。
    /// 幂等——已在册的跳过。intent_classifier 钉死关：前台会话由
    /// front 层光标管，kernel 不要自己按意图轮换 session。
    fn reconcile(&self) -> Result<(), String> {
        let names = self.workflow_names();
        for name in names {
            self.ensure_agent(&name)?;
        }
        Ok(())
    }

    /// workflows/ 下的目录名（跳过文件与隐藏目录），me 不在其中。
    fn workflow_names(&self) -> Vec<String> {
        let root = self.home.join("workflows");
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == SYSTEM_AGENT_ME || !e.path().is_dir() {
                    continue;
                }
                out.push(name);
            }
        }
        out.sort();
        out
    }

    /// 名字在册？不在就 spawn（workspace 按名字落位）。返回 agent id。
    fn ensure_agent(&self, name: &str) -> Result<carrier_types::agent::AgentId, String> {
        if let Some(entry) = self.kernel.registry.find_by_name(name) {
            return Ok(entry.id);
        }
        let mut manifest = AgentManifest::default();
        manifest.name = name.to_string();
        manifest.workspace = Some(self.kernel.config.agent_workspace_dir(name));
        // 前台会话连续性归 front 层（光标+账本回合号）；kernel 的意图
        // 轮换会把「换个话题」折成新 session，与 D9 会话语义打架。
        manifest.intent_classifier_enabled = Some(false);
        // kernel 的 identity 7 件套（USER/TOOLS/AGENTS/BOOTSTRAP/IDENTITY…）
        // 不落树——FS.md 的家根与助理形状都不认。母体的 SOUL/MEMORY 随
        // 镜像出厂树来（烤线整树拷），助理的性格由 create 面（soul 参数）写。
        manifest.generate_identity_files = false;
        if name == SYSTEM_AGENT_ME {
            // 母体 manifest 对齐 wiring::seed_system_me 的语义（那边是老
            // carrier 入口的种子；server 直调路径自己种，不背 carrier crate）
            manifest.display_name = "我".to_string();
            manifest.description = "母体 — 对主人是总管，对外是门面（家根身份）".to_string();
        }
        self.kernel
            .spawn_agent_with_parent(manifest, None, None)
            .map_err(|e| format!("spawn agent '{name}': {e}"))
    }

    /// create 面用：新助理进 kernel 在册（幂等）。前台文件夹由
    /// FrontDesk 管，两边各建各的。
    pub fn spawn_workflow(&self, name: &str) -> Result<(), String> {
        self.ensure_agent(name).map(|_| ())
    }

    /// 跑一轮（谁都可以是 me）。所有失败折进返回的 Done——协议同构：
    /// done 是唯一终帧，调用方只管回给前台。
    pub fn run_turn(&self, desk: &FrontDesk, agent: &str, text: &str) -> Done {
        let log = agent_log(&self.home, agent);

        // 上次半路死掉的残骸先清偿成合法形状（悬空调用、未收口轮），
        // 修复一次性落账、可审计；顺手拿下一回合号。
        let scan = crate::ledger::scan(&log);
        if let Err(e) = crate::ledger::repair(&log) {
            return Done::err("ledger", format!("cannot repair session log: {e}"));
        }
        if !scan.dangling.is_empty() || scan.open_turn.is_some() {
            let repaired = scan.dangling.len() + u64::from(scan.open_turn.is_some()) as usize;
            eprintln!(
                "ledger: repaired {repaired} frame(s) before turn {} ({} dangling tool_call, open_turn={:?})",
                scan.next_turn,
                scan.dangling.len(),
                scan.open_turn,
            );
        }
        let turn = scan.next_turn;

        // 铁律：先记账、再跑——模型可见即已记录
        let request = agi::Request {
            avatar: agent.to_string(),
            session: crate::front::SESSION_MAIN.to_string(),
            text: text.to_string(),
            turn,
        };
        if let Err(e) = crate::ledger::append(&log, &Frame::Request(request)) {
            return Done::err("ledger", format!("cannot append session log: {e}"));
        }

        // agent 解析：在册直用；不在册（create 后 server 未重启、或
        // workflows/ 新出现）按需 spawn——ensure 幂等。
        let agent_id = match self.ensure_agent(agent) {
            Ok(id) => id,
            Err(e) => {
                let d = Done::err("kernel", e).at_turn(turn);
                let _ = crate::ledger::append(&log, &Frame::Done(d.clone()));
                return d;
            }
        };

        // 观察者对准当前轮（工具帧落这本账、steer 回执盖这个回合号）
        self.observer.set_current(agent, turn);
        // steer 支线登记运行化身（与 end_turn 同在持轮线程手里）
        desk.begin_turn(agent);
        // 轮 future 必须 spawn 到 worker 上跑：kernel 轮路径里有
        // block_in_place + Handle::block_on 的同步桥（sender 历史富集），
        // 只认 worker 线程——block_on 直接 poll 到调用线程上必 panic。
        // 这里 block_on 只等 JoinHandle，JoinError（worker 侧 panic）折
        // 成 BootFailed 走 done，不让残骸悬着。
        let kernel = Arc::clone(&self.kernel);
        let khandle = kernel.get_kernel_handle();
        let owned = text.to_string();
        let joined = self.rt.block_on(async move {
            tokio::spawn(async move {
                kernel
                    .send_message_with_handle(
                        agent_id,
                        &owned,
                        khandle,
                        Some("front".to_string()),
                        None, None, None, None, None,
                    )
                    .await
            })
            .await
        });
        desk.end_turn();
        self.observer.clear_current();
        let result = joined.unwrap_or_else(|e| {
            Err(carrier_kernel::error::KernelError::BootFailed(format!(
                "turn task join failed: {e}"
            )))
        });

        let done = match result {
            Ok(r) => Done::ok(r.response).at_turn(turn),
            Err(e) => Done::err("kernel", e.to_string()).at_turn(turn),
        };
        let _ = crate::ledger::append(&log, &Frame::Done(done.clone()));
        done
    }
}

/// brain.json 桥：home/brain.json 缺席且 AGINX_BRAIN_URL 在场 → 合成
/// 最小档（一次性；此后文件是真源，env 只补 key）。两头都没有就放着
/// 让 kernel 自己报 BootFailed——不猜。
fn bridge_brain_json(home: &std::path::Path) -> Result<(), String> {
    let path = home.join("brain.json");
    if path.exists() {
        return Ok(());
    }
    let Ok(url) = std::env::var("AGINX_BRAIN_URL") else {
        return Ok(());
    };
    if url.trim().is_empty() {
        return Ok(());
    }
    let doc = serde_json::json!({
        "base_url": url,
        "api_key_env": "AGINXBRAIN_API_KEY",
        "default_modality": "chat",
        "modalities": { "chat": { "description": "front-desk chat (env bridge)" } },
    });
    std::fs::create_dir_all(home).map_err(|e| format!("brain bridge mkdir: {e}"))?;
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("brain bridge write: {e}"))?;
    eprintln!("mother: synthesized {}/brain.json from AGINX_BRAIN_URL (one-time bridge)", home.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{boot_host, now, oai, read_frames, stub_brain};

    /// 直调全生命周期：me 一轮（request → done）落在 home/sessions；
    /// 助理带工具的一轮（request → tool_call → tool_result → done）落在
    /// workflows/<名>/sessions——工具是未知名，kernel 判失败回账，
    /// call/result 照样成对（账面完整性不依赖工具好坏）。小满目录
    /// boot 前就位 → reconcile 在册路径。
    #[test]
    fn direct_turn_ledger_me_and_tool_flow() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-mother-direct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("workflows/小满")).unwrap();
        let addr = stub_brain(vec![
            now(oai("我在，我是母体。", None, "stop")),
            now(oai("我查一下", Some(r#"[{"id":"c1","type":"function","function":{"name":"no_such_tool_xyz","arguments":"{\"x\":1}"}}]"#), "tool_calls")),
            now(oai("查完了", None, "stop")),
        ]);
        let (desk, mother) = boot_host(&dir, &addr);

        // me 轮：直答，账在 home 根（母体不是文件夹）
        let d = mother.run_turn(&desk, MOTHER, "你是谁");
        assert!(d.ok, "me turn: {:?}", d.error);
        assert_eq!(d.text, "我在，我是母体。");
        let me_log = agent_log(&dir, MOTHER);
        let frames = read_frames(&me_log);
        assert_eq!(frames.len(), 2);
        assert!(matches!(&frames[0], Frame::Request(r) if r.text == "你是谁" && r.turn == 1 && r.avatar == "me"));
        assert!(matches!(&frames[1], Frame::Done(x) if x.ok && x.turn == Some(1)));

        // 助理轮：工具帧经观察者落账
        let d = mother.run_turn(&desk, "小满", "打个招呼");
        assert!(d.ok, "assistant turn: {:?}", d.error);
        assert_eq!(d.text, "查完了");
        let log = agent_log(&dir, "小满");
        let frames = read_frames(&log);
        assert_eq!(frames.len(), 4, "frames: {frames:?}");
        assert!(matches!(&frames[0], Frame::Request(r) if r.text == "打个招呼" && r.turn == 1));
        assert!(matches!(&frames[1], Frame::ToolCall(c) if c.id == "c1" && c.tool == "no_such_tool_xyz"));
        assert!(matches!(&frames[2], Frame::ToolResult(r) if r.id == "c1" && !r.ok && !r.err.is_empty()));
        assert!(matches!(&frames[3], Frame::Done(x) if x.ok && x.turn == Some(1)));

        // kernel 侧真在册：me 与 小满 都有 agent
        assert!(mother.kernel.registry.find_by_name("me").is_some());
        assert!(mother.kernel.registry.find_by_name("小满").is_some());
    }

    /// 出厂树契约（结构刀④）：boot 不种不碰家根——SOUL/MEMORY 随镜像
    /// 出厂树来（烤线整树拷，真源=仓里 home/），用户文件 boot 后一字
    /// 不动；me 跑过一轮后家根干净——kernel 的 identity 脚手架
    /// （USER/TOOLS/AGENTS/BOOTSTRAP/IDENTITY/HEARTBEAT）一个都不落树，
    /// 助理目录同理（助理的性格走 create 面的 soul 参数，不走脚手架）。
    #[test]
    fn home_untouched_by_boot_and_no_identity_junk() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-mother-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("workflows/小满")).unwrap();
        std::fs::write(dir.join("SOUL.md"), "用户改过的灵魂").unwrap();
        std::fs::write(dir.join("MEMORY.md"), "用户的记忆索引").unwrap();

        let addr = stub_brain(vec![now(oai("答", None, "stop"))]);
        let (desk, mother) = boot_host(&dir, &addr);

        // 契约：boot 只保 sessions/，家根文件原样
        assert_eq!(std::fs::read_to_string(dir.join("SOUL.md")).unwrap(), "用户改过的灵魂");
        assert_eq!(std::fs::read_to_string(dir.join("MEMORY.md")).unwrap(), "用户的记忆索引");
        assert!(dir.join("sessions").is_dir());

        let d = mother.run_turn(&desk, MOTHER, "你是谁");
        assert!(d.ok, "me turn: {:?}", d.error);

        // 家根与助理目录都不得出现 kernel identity 脚手架
        for root in [&dir, &dir.join("workflows/小满")] {
            for junk in ["USER.md", "TOOLS.md", "AGENTS.md", "BOOTSTRAP.md", "IDENTITY.md", "HEARTBEAT.md"] {
                assert!(!root.join(junk).exists(), "identity junk leaked: {}/{}", root.display(), junk);
            }
        }
    }

    /// 残账修复：上次死在半路（悬空 tool_call、request 未收口），
    /// 下一次 run_turn 先补账再开新轮，回合号自动续。
    #[test]
    fn crash_residue_repaired_before_next_turn() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-mother-repair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("workflows/小满/sessions")).unwrap();
        let addr = stub_brain(vec![now(oai("答", None, "stop"))]);
        let (desk, mother) = boot_host(&dir, &addr);

        let log = agent_log(&dir, "小满");
        std::fs::write(
            &log,
            concat!(
                r#"{"t":"request","avatar":"小满","session":"main","text":"旧问题","turn":0}"#, "\n",
                r#"{"t":"tool_call","id":"c9","tool":"dev-hello","args":["x"]}"#, "\n",
            ),
        )
        .unwrap();

        let d = mother.run_turn(&desk, "小满", "新问题");
        assert!(d.ok);
        let frames = read_frames(&log);
        // 旧账 2 + 补账 2（悬空结果 + synthetic done）+ 新轮 2（request+done）
        assert_eq!(frames.len(), 6);
        assert!(matches!(&frames[2], Frame::ToolResult(r) if r.id == "c9" && !r.ok));
        assert!(matches!(&frames[3], Frame::Done(x) if !x.ok && x.turn == Some(0)));
        assert!(matches!(&frames[4], Frame::Request(r) if r.text == "新问题" && r.turn == 2));
        assert!(matches!(&frames[5], Frame::Done(x) if x.ok && x.turn == Some(2)));
    }

    /// brain 桥：home 无 brain.json 且 env 有 AGINX_BRAIN_URL → 合成；
    /// 已有文件不动；两头都没有 → 不猜（留给 kernel 报）。
    #[test]
    fn brain_json_bridge_rules() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-mother-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 已有文件：不动
        std::fs::write(dir.join("brain.json"), r#"{"existing":true}"#).unwrap();
        bridge_brain_json(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("brain.json")).unwrap(), r#"{"existing":true}"#);

        // 无文件 + env 在场 → 合成
        std::fs::remove_file(dir.join("brain.json")).unwrap();
        std::env::set_var("AGINX_BRAIN_URL", "http://127.0.0.1:9/v1/chat/completions");
        bridge_brain_json(&dir).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("brain.json")).unwrap()).unwrap();
        assert_eq!(doc["base_url"], "http://127.0.0.1:9/v1/chat/completions");
        assert_eq!(doc["api_key_env"], "AGINXBRAIN_API_KEY");
        assert_eq!(doc["default_modality"], "chat");
        std::env::remove_var("AGINX_BRAIN_URL");

        // 无文件 + env 缺席 → 不合成
        std::fs::remove_file(dir.join("brain.json")).unwrap();
        bridge_brain_json(&dir).unwrap();
        assert!(!dir.join("brain.json").exists());
    }

    /// 账本路径律：me=home/sessions，助理=home/workflows/<名>/sessions。
    #[test]
    fn agent_log_layout() {
        let home = std::path::Path::new("/home");
        assert_eq!(agent_log(home, "me"), PathBuf::from("/home/sessions/main.jsonl"));
        assert_eq!(
            agent_log(home, "小满"),
            PathBuf::from("/home/workflows/小满/sessions/main.jsonl")
        );
    }
}
