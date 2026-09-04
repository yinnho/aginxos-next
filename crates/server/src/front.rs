// front — 前台（宪法 D10/D11）。登记语义：进/住/切/退。
//
// - 光标是纯内存状态：开机 = 母体 me（D10），重启即回 me——这不是丢失，
//   这就是语义（退房是登记行为，不是数据）。
// - 花名册 = workspaces 目录清单派生（D5：化身 = 文件夹，目录即注册）。
// - 母体 me 不是文件夹，是前台里的一段代码（见 mother.rs）。
// - 一次一轮：前台只有一张嘴（单用户手机的物理事实），send 全程持
//   turn 锁，后来的连线排队等——语音/CLI/未来的 webhook 都一样。

use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

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

pub struct FrontDesk {
    /// workspaces 根（AGINX_HOME 下的 workspaces/）。
    root: PathBuf,
    cursor: Mutex<String>,
    turn: Mutex<()>,
}

impl FrontDesk {
    pub fn new(root: PathBuf) -> FrontDesk {
        FrontDesk { root, cursor: Mutex::new(MOTHER.to_string()), turn: Mutex::new(()) }
    }

    /// 当前住台的化身（"me" = 母体）。
    pub fn cursor(&self) -> String {
        self.cursor.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set_cursor(&self, who: &str) {
        *self.cursor.lock().unwrap_or_else(|p| p.into_inner()) = who.to_string();
    }

    /// 全局一轮锁。拿到才许开跑一轮（母体直答或化身 spawn 都算）。
    pub fn turn_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(|p| p.into_inner())
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
}
