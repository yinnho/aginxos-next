// front — 前台（宪法 D10/D11）。登记语义：进/住/切/退。
//
// - 光标是纯内存状态：开机 = 母体 me（D10），重启即回 me——这不是丢失，
//   这就是语义（退房是登记行为，不是数据）。
// - 花名册 = workspaces 目录清单派生（D5：化身 = 文件夹，目录即注册）。
// - 母体 me 不是文件夹，是前台里的一段代码（见 mother.rs）。
// - 一次一轮：前台只有一张嘴（单用户手机的物理事实），send 全程持
//   turn 锁，后来的连线排队等——语音/CLI/未来的 webhook 都一样。
//   ② steer 支线：目标化身正有轮在跑时，后到的 send 不排队，插进那
//   轮的下一个工具步边界（dsh 词表：运行中插入=steer，空闲插入=下一
//   回合）；插不进（母体直答/退房/换化身/轮正好收尾）照旧排队。

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};

/// v0 每化身一个常驻会话；多会话（D10 切会话）后续按 sessions/ 清单加。
pub const SESSION_MAIN: &str = "main";

/// 母体的名字。既是光标的默认值，也是 `aginx agent send me …` 的目标。
pub const MOTHER: &str = "me";

/// 退房词（D10 退）：整段完全匹配才算——正文里顺带提到不退房。
/// 与 M42a 语音封闭词表同源，host v0 收敛到这一小撮。
pub const CHECKOUT_WORDS: &[&str] =
    &["再见", "退下", "回去吧", "拜拜", "回母体", "退房", "bye", "goodbye"];

pub fn is_checkout_word(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    CHECKOUT_WORDS.contains(&t.as_str())
}

// ---------------- ② steer 支线 ----------------
//
// 运行中轮的插入队列。持轮线程（relay 循环）是队列唯一消费者：每个
// 工具回账后 take_steers 下发；轮收口时 end_turn 把没来得及下发的
// 一律 TurnEnded——发送方拿到后回头抢锁，那句话重分类为下一回合
// （dsh：唤醒输入撞上已收口活动 = 下一回合）。同一把 steer 锁里同时
// 放 running 标记，保证「入队成功」和「有人负责善后」在同一个临界
// 区内闭合：begin/end 都发生在持轮线程手里，不存在入了队却没人
// 善后的窗口。

/// steer 发送方的回执：Delivered(回合号) = 已落账并写进 runtime 的
/// 管；TurnEnded = 轮先收口了，没插进去。
pub enum SteerOutcome {
    Delivered(u64),
    TurnEnded,
}

/// 一笔待插的主动输入。
pub(crate) struct SteerReq {
    pub(crate) text: String,
    pub(crate) reply: mpsc::Sender<SteerOutcome>,
}

#[derive(Default)]
struct SteerBox {
    /// 正在跑轮的化身（None = 空闲；母体直答不算——没账没轮可插）。
    running: Option<String>,
    queue: Vec<SteerReq>,
}

pub struct FrontDesk {
    /// workspaces 根（AGINX_HOME 下的 workspaces/）。
    root: PathBuf,
    cursor: Mutex<String>,
    turn: Mutex<()>,
    steer: Mutex<SteerBox>,
}

impl FrontDesk {
    pub fn new(root: PathBuf) -> FrontDesk {
        FrontDesk {
            root,
            cursor: Mutex::new(MOTHER.to_string()),
            turn: Mutex::new(()),
            steer: Mutex::new(SteerBox::default()),
        }
    }

    /// 当前住台的化身（"me" = 母体）。
    pub fn cursor(&self) -> String {
        self.cursor.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set_cursor(&self, who: &str) {
        *self.cursor.lock().unwrap_or_else(|p| p.into_inner()) = who.to_string();
    }

    /// 全局一轮锁。拿到才许开跑一轮（母体直答或化身 spawn 都算）。
    pub fn turn_lock(&self) -> MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// ② 不排队的轮锁探测：Some = 空闲可开跑，None = 有轮在跑。
    pub fn try_turn(&self) -> Option<MutexGuard<'_, ()>> {
        self.turn.try_lock().ok()
    }

    /// ② 登记运行化身（持轮线程在 relay 前调）。此后同目标的 send 走
    /// steer 入队而不是排队。
    pub fn begin_turn(&self, avatar: &str) {
        self.steer.lock().unwrap_or_else(|p| p.into_inner()).running = Some(avatar.to_string());
    }

    /// ② 轮收口：清运行标记，余下排队 steer 一律 TurnEnded。
    pub fn end_turn(&self) {
        let mut b = self.steer.lock().unwrap_or_else(|p| p.into_inner());
        b.running = None;
        for r in std::mem::take(&mut b.queue) {
            let _ = r.reply.send(SteerOutcome::TurnEnded);
        }
    }

    /// ② relay 的步边界取走排队 steer（只有持轮的 relay 会调）。
    pub fn take_steers(&self) -> Vec<SteerReq> {
        std::mem::take(&mut self.steer.lock().unwrap_or_else(|p| p.into_inner()).queue)
    }

    /// ② 目标化身正有轮在跑 → 入队，返回回执线；否则 None（调用方走
    /// 正门排队当下一回合）。
    pub fn push_steer(&self, avatar: &str, text: &str) -> Option<mpsc::Receiver<SteerOutcome>> {
        let (tx, rx) = mpsc::channel();
        let mut b = self.steer.lock().unwrap_or_else(|p| p.into_inner());
        if b.running.as_deref() != Some(avatar) {
            return None;
        }
        b.queue.push(SteerReq { text: text.to_string(), reply: tx });
        Some(rx)
    }

    /// 花名册：workspaces 下的目录清单，字典序。母体不在册（不是文件夹）。
    pub fn roster(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }

    pub fn avatar_exists(&self, name: &str) -> bool {
        self.root.join(name).is_dir()
    }

    /// 进（建化身 = 建文件夹，D5）：sessions/ + output/ 起手，SOUL.md 可选。
    pub fn create_avatar(&self, name: &str, soul: Option<&str>) -> io::Result<PathBuf> {
        validate_name(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let ws = self.root.join(name);
        if ws.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("avatar '{name}' already exists"),
            ));
        }
        std::fs::create_dir_all(ws.join("sessions"))?;
        std::fs::create_dir_all(ws.join("output"))?;
        if let Some(soul) = soul.map(str::trim).filter(|s| !s.is_empty()) {
            std::fs::write(ws.join("SOUL.md"), soul)?;
        }
        Ok(ws)
    }

    /// send 的目标裁决（D10）：
    /// - 退房词优先于一切：说退房就是退房，回母体。
    /// - 显式点名（进/切）：光标落到该化身；不存在 = NotFound（前台不
    ///   顺便造人——建化身是 create，说话是 send，两件事分开）。
    /// - 不点名（住）：光标是谁就给谁；开机状态落在母体。
    pub fn resolve_send(&self, explicit: Option<&str>, text: &str) -> Result<SendTarget, String> {
        if is_checkout_word(text) {
            self.set_cursor(MOTHER);
            return Ok(SendTarget::Checkout);
        }
        match explicit {
            Some(name) if name == MOTHER => {
                self.set_cursor(MOTHER);
                Ok(SendTarget::Mother)
            }
            Some(name) => {
                if !self.avatar_exists(name) {
                    return Err(format!("unknown avatar '{name}'"));
                }
                self.set_cursor(name);
                Ok(SendTarget::Avatar(name.to_string()))
            }
            None => {
                let cur = self.cursor();
                if cur == MOTHER {
                    Ok(SendTarget::Mother)
                } else {
                    Ok(SendTarget::Avatar(cur))
                }
            }
        }
    }
}

pub enum SendTarget {
    Mother,
    Avatar(String),
    Checkout,
}

/// 化身名规则：非空、不许路径分隔符/点开头、不许叫 me（那是母体）。
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("avatar name is empty".into());
    }
    if name == MOTHER {
        return Err("'me' is the mother, not an avatar".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(format!("avatar name must not contain path separators: '{name}'"));
    }
    if name.starts_with('.') {
        return Err(format!("avatar name must not start with '.': '{name}'"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每测试独立目录：pid 键会让同进程并行测试共享一棵树，create 撞名。
    fn desk(tag: &str) -> (FrontDesk, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "aginx-server-test-front-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("workspaces")).unwrap();
        (FrontDesk::new(dir.join("workspaces")), dir)
    }

    #[test]
    fn checkout_words_exact_match_only() {
        assert!(is_checkout_word("再见"));
        assert!(is_checkout_word(" Bye ")); // 大小写+空白宽容
        assert!(!is_checkout_word("再见，明天聊"));
        assert!(!is_checkout_word("goodbye world"));
        assert!(!is_checkout_word("你好"));
    }

    #[test]
    fn boot_cursor_is_mother_and_stay_routes_to_cursor() {
        let (d, _dir) = desk("boot");
        assert_eq!(d.cursor(), MOTHER);
        assert!(matches!(d.resolve_send(None, "你好"), Ok(SendTarget::Mother)));
    }

    #[test]
    fn register_routes_and_switches() {
        let (d, _dir) = desk("routes");
        d.create_avatar("小满", None).unwrap();
        assert_eq!(d.roster(), vec!["小满"]);
        // 进/切：点名设光标
        assert!(matches!(d.resolve_send(Some("小满"), "在吗"), Ok(SendTarget::Avatar(n)) if n == "小满"));
        assert_eq!(d.cursor(), "小满");
        // 住：不点名给当前光标
        assert!(matches!(d.resolve_send(None, "继续"), Ok(SendTarget::Avatar(n)) if n == "小满"));
        // 点名 me：显式回母体
        assert!(matches!(d.resolve_send(Some("me"), "hi"), Ok(SendTarget::Mother)));
        assert_eq!(d.cursor(), MOTHER);
    }

    #[test]
    fn checkout_returns_cursor_to_mother() {
        let (d, _dir) = desk("checkout");
        d.create_avatar("小满", None).unwrap();
        d.resolve_send(Some("小满"), "在吗").unwrap();
        assert!(matches!(d.resolve_send(None, "再见"), Ok(SendTarget::Checkout)));
        assert_eq!(d.cursor(), MOTHER);
    }

    #[test]
    fn unknown_avatar_is_rejected_not_created() {
        let (d, _dir) = desk("unknown");
        assert!(d.resolve_send(Some("阿宝"), "hi").is_err());
        assert!(!d.avatar_exists("阿宝"));
        assert_eq!(d.cursor(), MOTHER); // 点名失败不动光标
    }

    #[test]
    fn create_validates_names() {
        let (d, _dir) = desk("create");
        assert!(d.create_avatar("", None).is_err());
        assert!(d.create_avatar("me", None).is_err());
        assert!(d.create_avatar("../escape", None).is_err());
        assert!(d.create_avatar(".hidden", None).is_err());
        let ws = d.create_avatar("小满", Some("  你是小满。  ")).unwrap();
        assert_eq!(std::fs::read_to_string(ws.join("SOUL.md")).unwrap(), "你是小满。");
        assert!(ws.join("sessions").is_dir());
        assert!(ws.join("output").is_dir());
    }

    /// ② steer 队列生命周期：空闲不入队、同名入队异名拒、relay 取走后
    /// 回执、收口冲掉余量、收口后不再入队。
    #[test]
    fn steer_box_lifecycle() {
        let (d, _dir) = desk("steer");
        // 空闲：没有轮在跑，谁都插不进
        assert!(d.push_steer("小满", "x").is_none());
        d.create_avatar("小满", None).unwrap();
        // 轮跑起来：同名入队，异名（换化身）不入
        d.begin_turn("小满");
        let rx = d.push_steer("小满", "改成上海").unwrap();
        assert!(d.push_steer("阿宝", "x").is_none());
        // relay 步边界取走 → 回执 Delivered 带回合号；取空后再取为空
        let steers = d.take_steers();
        assert_eq!(steers.len(), 1);
        assert_eq!(steers[0].text, "改成上海");
        steers[0].reply.send(SteerOutcome::Delivered(7)).unwrap();
        assert!(matches!(rx.recv(), Ok(SteerOutcome::Delivered(7))));
        assert!(d.take_steers().is_empty());
        // 轮收口：还排着的冲 TurnEnded；此后不再入队
        let rx2 = d.push_steer("小满", "晚了一步").unwrap();
        d.end_turn();
        assert!(matches!(rx2.recv(), Ok(SteerOutcome::TurnEnded)));
        assert!(d.push_steer("小满", "x").is_none());
    }
}
