//! 语音对话协议 v0 — 无 LLM 的确定性状态机（M42a，命令优先重设计 2026-09-06）。
//!
//! 产品定义（2026-09-05 终稿）：手机即智能体，人只发指令、机器干活——
//! 缺参数才问，一次问一个，问完就干。老「扫描→序数→拼密码→回读确认」
//! 多步流程废弃：连网只剩「说连网」或「对准码」，机器自己走完剩下的一
//! 切（配对码是超集：WiFi + brain 钥匙 + 网关身份 + relay 密一眼全落）。
//! 没听懂就说没听懂，不做模糊匹配——这是自举地板：WiFi 必须在 LLM 可用
//! 之前连得上。
//!
//! 纯逻辑、零 I/O：daemon（main.rs）喂 Ev，收 Out；所有副作用（TTS、
//! 拍照、连接、配对落盘）都是 Out::Act，由 daemon 落地。机器的下一步
//! 也由协议决定（NetState::NoConf → 直接开眼等码），daemon 只执行。

// ---------------- events / outputs ----------------

/// 网络现状（daemon 查完回喂；连网命令的判定输入）。
#[derive(Debug, Clone, PartialEq)]
pub enum NetState {
    /// wlan0 有 IPv4——什么都不用做
    Up,
    /// 有 wifi.conf 但按它连不上（密码换了/AP 没了）——开眼等码
    ConfFail,
    /// 没有任何身份记录——开眼等码
    NoConf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    /// 一段 ASR 识别文本（已归一化由本模块完成，原样传入即可）
    Heard(String),
    /// Act::NetConnect 的判定结果（daemon 查 IP/conf 后回喂）
    NetState(NetState),
    /// Act::Join 完成：Ok(ip) / Err(原因)——WIFI: 码路径
    JoinDone(Result<String, String>),
    /// Act::PairApply 完成：Ok(报告话) / Err(原因)——配对码路径
    PairDone(Result<String, String>),
    /// Act::QrScan 完成：Ok(每码一条 payload) / Err(拍照/解码失败原因)
    QrDone(Result<Vec<String>, String>),
    /// Act::Ocr 完成：Ok(每行一条文本) / Err(拍照/识别失败原因)
    OcrDone(Result<Vec<String>, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// 连网：daemon 查 IP/wifi.conf 后回 NetState
    NetConnect,
    /// 加入网络（WIFI: 码到手，直接连——拉式）
    Join { ssid: String, psk: String },
    /// 配对落盘（AGINXPAIR1 码到手：连网 + 身份三件进 env + 服务重启，
    /// 拉式到手即用，不回读不确认���
    PairApply { bundle: aginx_qr::PairBundle },
    /// 打开眼取景（与音量+同一条路；已开则空转）
    Eye,
    /// 关闭眼取景（取消语义的落地点；没开则空转）
    EyeClose,
    /// 拍照解 QR（cam-shot 盲拍 + aginx-qr，M42b 眼分支）
    QrScan,
    /// 拍照念字（cam-shot 盲拍 + ag-ocr，M45 眼分支）
    Ocr,
    /// 状态查询（时间/电池/IP）——daemon 读系统后经 inject_say 出声
    Status,
    /// 自由文本喂母体/新前台（N2②：aginx agent send）。封闭词表全部
    /// miss 时走这里；前台不可达由 daemon 落回离线地板话。
    Chat(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// 机器话语：上屏（对话行）。拉式语音（2026-09-04 用户收据：识别和
    /// 回应都快、唯独嘴慢）——daemon 对 Say 默认不出声，回应即时上脸；
    /// 想听再说「你说给我听」。
    Say(String),
    /// 点名要听：上屏（若文本还不在屏上，调用方先推行）+ 必 TTS。
    /// 「念一下」的 OCR 读、「再念一遍」的回读、「你说给我听」的复述。
    Speak(String),
    /// 只刷屏不出声（状态变了但不需要说）
    Show,
    /// 执行动作
    Act(Act),
}

// ---------------- state ----------------

/// 对话机（M42a 命令优先版）。全流程无驻留态：每个指令一步走完，
/// 等码的长等待由眼取景自己的生命周期管（30s 上限 + 帧自愈），协议
/// 不再记「现在在第几步」。state_name 留作屏显与未来状态（点选纠错
/// 的 Choice 态）的接入缝。
pub struct Vm {
    /// 对话行（(谁, 文本)），屏显用；cap 8 行滚动
    lines: Vec<(bool, String)>,
    /// 自由文本改投新前台（N2②）。false = 老行为（没听懂地板）。
    /// new() 恒 false——既有测试不动，开前台一律 with_front()。
    front: bool,
}

impl Vm {
    pub fn new() -> Vm {
        Vm {
            lines: Vec::new(),
            front: false,
        }
    }

    /// 开前台的 Vm：step_idle 的自由文本分支改投 Act::Chat。
    pub fn with_front() -> Vm {
        Vm {
            front: true,
            ..Vm::new()
        }
    }

    pub fn state_name(&self) -> &'static str {
        "idle"
    }

    /// 屏显对话行：(is_user, text)
    pub fn lines(&self) -> &[(bool, String)] {
        &self.lines
    }

    /// 状态查询走 Act::Status：daemon 读系统（时间/电池/IP）后用 inject_say
    /// 把拼好的话送回来出声（SM 纯逻辑，不碰钟）。
    pub fn inject_say(&mut self, s: &str) -> Out {
        self.lines.push((false, s.to_string()));
        self.trim_lines();
        Out::Say(s.to_string())
    }

    fn say(&mut self, outs: &mut Vec<Out>, s: &str) {
        self.lines.push((false, s.to_string()));
        self.trim_lines();
        outs.push(Out::Say(s.to_string()));
    }
    /// 点名要听的话语：推对话行 + 必出声。
    fn say_loud(&mut self, outs: &mut Vec<Out>, s: &str) {
        self.lines.push((false, s.to_string()));
        self.trim_lines();
        outs.push(Out::Speak(s.to_string()));
    }
    fn heard_line(&mut self, s: &str) {
        self.lines.push((true, s.to_string()));
        self.trim_lines();
    }
    fn trim_lines(&mut self) {
        while self.lines.len() > 8 {
            self.lines.remove(0);
        }
    }

    /// 驱动一步。返回要 daemon 落地的输出序列。
    pub fn step(&mut self, ev: Ev) -> Vec<Out> {
        let mut outs = Vec::new();
        match ev {
            Ev::Heard(raw) => {
                let text = norm(&raw);
                if text.is_empty() {
                    // 空 ASR 不静默：走到这的都是 ≥0.1s 的真尝试（真误触在
                    // capture_take 已拦）。不吭声=用户以为死了（2026-09-04
                    // 三连空串实测）。也不进对话行——屏上留空行难看。
                    self.say(&mut outs, "没听清。请按住键说。");
                    outs.push(Out::Show);
                    return outs;
                }
                self.heard_line(&raw);
                // 取消优先于一切
                if is_cancel(&text) {
                    self.say(&mut outs, "已取消。");
                    outs.push(Out::Act(Act::EyeClose));
                    outs.push(Out::Show);
                    return outs;
                }
                // 点名要听（拉式语音）：复述最近一条机器话语。
                if is_speak_request(&text) {
                    match self.lines.iter().rev().find(|(u, _)| !u) {
                        Some((_, s)) => outs.push(Out::Speak(s.clone())),
                        None => self.say_loud(&mut outs, "我还没说过话。"),
                    }
                    outs.push(Out::Show);
                    return outs;
                }
                self.step_idle(&text, &mut outs);
            }
            Ev::NetState(ns) => match ns {
                NetState::Up => self.say(&mut outs, "网已连。"),
                NetState::ConfFail | NetState::NoConf => {
                    // 机器自己走下一步：不开清单不问人，直接睁眼等码。
                    // 配对码是超集（连网+身份），WIFI: 码只连网，都是拉式。
                    self.say(&mut outs, "对准配对码或无线码。");
                    outs.push(Out::Act(Act::Eye));
                }
            },
            Ev::JoinDone(r) => match r {
                Ok(ip) => self.say(&mut outs, &format!("连上了，地址{ip}。")),
                Err(e) => self.say(&mut outs, &format!("没连上，{e}。再说连网重试。")),
            },
            Ev::PairDone(r) => match r {
                Ok(msg) => {
                    // 脸上留技术细节；收尾句出声（开机体验定档：hardline
                    // 接回 = 母体重新接通，Matrix 法理即技术真相）。
                    self.say(&mut outs, &format!("配对完成，{msg}。"));
                    self.say_loud(&mut outs, "Connection restored. Welcome to the real world.");
                }
                Err(e) => self.say(&mut outs, &format!("配对没成，{e}。再说连网重试。")),
            },
            Ev::QrDone(r) => {
                // 拉式：取景器开着就是机器在等码——到手直接用，不问「是
                // 不是」。配对码是 WiFi 码的超集，先配对后 WiFi；回读确认
                // 只留给耳朵听来的（噪声通道）。psk 一律不上屏。
                let pair = match &r {
                    Ok(payloads) => payloads
                        .iter()
                        .find_map(|p| aginx_qr::parse_pair_payload(p)),
                    Err(_) => None,
                };
                let wifi = match &r {
                    Ok(payloads) => payloads
                        .iter()
                        .find_map(|p| aginx_qr::parse_wifi_payload(p)),
                    Err(_) => None,
                };
                match (pair, wifi) {
                    (Some(bundle), _) => {
                        let ask = format!(
                            "扫到{}的配对码，连接配对。",
                            bundle.ssid
                        );
                        self.say(&mut outs, &ask);
                        outs.push(Out::Act(Act::PairApply { bundle }));
                    }
                    (None, Some(w)) => {
                        let ask = if w.psk.is_empty() {
                            format!("扫到网络{}，开放网络，连接。", w.ssid)
                        } else {
                            format!(
                                "扫到网络{}，密码{}位，连接。",
                                w.ssid,
                                w.psk.chars().count()
                            )
                        };
                        self.say(&mut outs, &ask);
                        outs.push(Out::Act(Act::Join {
                            ssid: w.ssid,
                            psk: w.psk,
                        }));
                    }
                    (None, None) => match r {
                        Ok(payloads) => match payloads.into_iter().next() {
                            // 非密码类码（URL/文本）：念前 40 字，全文进
                            // 对话行（屏可看全）
                            Some(p) => {
                                let head: String = p.chars().take(40).collect();
                                // 截断以 … 收尾，未截断以句号收尾
                                let tail = if p.chars().count() > 40 {
                                    "…".to_string()
                                } else {
                                    "。".to_string()
                                };
                                self.heard_line(&p);
                                self.say(&mut outs, &format!("扫到，{head}{tail}"));
                            }
                            None => self.say(&mut outs, "没拍到二维码，正对着它再说扫码。"),
                        },
                        Err(_) => self.say(&mut outs, "没拍到二维码，正对着它再说扫码。"),
                    },
                }
                outs.push(Out::Show);
            }
            Ev::OcrDone(r) => {
                // M45 眼分支：识别行全文上屏（每行一条对话行，滚动窗取尾），
                // 语音走 Say。短文整篇念（daemon 侧 split_clauses 分句流水）；
                // 长文（>120 字）只念前两行——耳朵听不下整页，眼睛看屏。
                match r {
                    Ok(lines) if !lines.is_empty() => {
                        for l in &lines {
                            self.lines.push((false, l.clone()));
                        }
                        self.trim_lines();
                        // 念一下本身就是要听（拉式语音的点名面）：Speak 只
                        // 出声不再推行。
                        let joined = lines.join("。");
                        if joined.chars().count() > 120 {
                            let head =
                                lines.iter().take(2).cloned().collect::<Vec<_>>().join("。");
                            outs.push(Out::Speak(format!("{head}。全文在屏幕上。")));
                        } else {
                            outs.push(Out::Speak(joined));
                        }
                    }
                    Ok(_) | Err(_) => {
                        self.say(&mut outs, "没拍到文字，正对着它再说念一下。");
                    }
                }
                outs.push(Out::Show);
            }
        }
        outs
    }

    fn step_idle(&mut self, text: &str, outs: &mut Vec<Out>) {
        if is_scan(text) {
            // is_scan 判在 is_wifi 之前：「扫码连无线」含「无线」，但主动词
            // 是相机——口语里说要扫码，给相机。
            self.say(outs, "拍照扫码，对准二维码别动。");
            outs.push(Out::Act(Act::QrScan));
        } else if is_ocr(text) {
            // 念读也判在 is_wifi 前（「念一下」类词不含网络词，纯判序保守）
            self.say(outs, "拍照念字，对准文字别动。");
            outs.push(Out::Act(Act::Ocr));
        } else if is_wifi(text) {
            self.say(outs, "看一下网络。");
            outs.push(Out::Act(Act::NetConnect));
        } else if is_status(text) {
            self.say(outs, "看一下。");
            outs.push(Out::Act(Act::Status));
        } else if is_hello(text) {
            self.say(outs, "我在。说连网，或说扫码、念一下。");
        } else if is_help(text) {
            self.say(
                outs,
                "我能连网、扫码、念字。回应都在屏幕上，要听我说，说你说给我听。",
            );
        } else {
            if self.front {
                // N2②：封闭词表 miss = 自由文本 → 母体（新前台）。封闭
                // 词表本身仍本地优先（连网/扫码/念读是离线地板，不进
                // brain 往返）；前台不可达由 daemon 收兜底话，这里不降级。
                self.say(outs, "问母体，稍等。");
                outs.push(Out::Act(Act::Chat(text.to_string())));
            } else {
                self.say(outs, "没听懂。说连网，或扫码，或念一下。");
            }
        }
        outs.push(Out::Show);
    }
}

// ---------------- 归一化 ----------------

/// ASR 后处理归一化：去标点/空白，全角转半角。保留大小写（拼读语义在大小写）。
/// 例："连接无线。第三个密码，大写A小写B数字3。" → "连接无线第三个密码大写A小写B数字3"
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        // 全角 ASCII → 半角
        let ch = match ch as u32 {
            0xFF01..=0xFF5E => char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch),
            0x3000 => ' ',
            _ => ch,
        };
        // 标点（中英）与空白全去
        if ch.is_whitespace() {
            continue;
        }
        if matches!(
            ch,
            '。' | '，'
                | '、'
                | '！'
                | '？'
                | '：'
                | '；'
                | '…'
                | '—'
                | '·'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '《'
                | '》'
                | ','
                | '.'
                | '!'
                | '?'
                | ':'
                | ';'
                | '-'
                | '_'
                | '\''
                | '"'
                | '('
                | ')'
                | '<'
                | '>'
                | '+'
                | '='
                | '*'
                | '&'
                | '#'
                | '@'
                | '$'
                | '%'
                | '|'
                | '\\'
                | '/'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '~'
                | '`'
        ) {
            continue;
        }
        out.push(ch);
    }
    out
}

// ---------------- 词表匹配 ----------------

fn contains_any(lower: &str, words: &[&str]) -> bool {
    words.iter().any(|w| lower.contains(w))
}

fn is_wifi(t: &str) -> bool {
    let l = t.to_lowercase();
    // 裸"网"不收（"网站""网游"全误触）；wi+fi 拆开是 ASR 常态
    contains_any(
        &l,
        &[
            "无线",
            "网络",
            "连网",
            "上网",
            "wifi",
            "wi-fi",
            "wi fi",
            "连一下",
        ],
    ) || (l.contains("wi") && l.contains("fi"))
}

fn is_status(t: &str) -> bool {
    contains_any(t, &["状态", "时间", "几点", "电池", "电量", "ip", "地址"])
}

fn is_hello(t: &str) -> bool {
    contains_any(t, &["你在吗", "在吗", "你好", "喂"])
}

fn is_help(t: &str) -> bool {
    contains_any(t, &["帮助", "能做什么", "你会什么", "怎么用"])
}

fn is_cancel(t: &str) -> bool {
    contains_any(t, &["取消", "算了", "不弄了", "退出", "停止", "不干了"])
}

/// 点名要听（拉式语音的总开关词）。任何状态说这些词，都复述最近一条
/// 机器话语。「念给我听」不收：那是 OCR 的拍+念（is_ocr），同词不同义。
fn is_speak_request(t: &str) -> bool {
    contains_any(
        t,
        &[
            "你说给我听",
            "说给我听",
            "再说一遍",
            "再念一遍",
            "念一遍",
            "重复",
        ],
    )
}

fn is_scan(t: &str) -> bool {
    // 相机主动词。与口语「重新扫」不撞：重扫的诉求已由「连网」重新
    // 开眼覆盖（机器自己会再试）。
    contains_any(t, &["扫码", "二维码", "扫一扫", "扫一下", "扫个码"])
}

fn is_ocr(t: &str) -> bool {
    // M45 念读主动词。与 is_speak_request（念一遍/再说一遍——复述机器话）
    // 不撞：「念一下」≠「念一遍」。裸「念」「读」不收（「念念不忘」误触）。
    contains_any(
        t,
        &["念一下", "读一下", "念文字", "读文字", "念给我听", "这是什么字"],
    )
}

// ---------------- tests ----------------

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(vm: &mut Vm, s: &str) -> Vec<Out> {
        vm.step(Ev::Heard(s.into()))
    }
    fn says(outs: &[Out]) -> Vec<String> {
        outs.iter()
            .filter_map(|o| match o {
                Out::Say(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }
    fn speaks(outs: &[Out]) -> Vec<String> {
        outs.iter()
            .filter_map(|o| match o {
                Out::Speak(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }
    fn acts(outs: &[Out]) -> Vec<Act> {
        outs.iter()
            .filter_map(|o| match o {
                Out::Act(a) => Some(a.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn norm_strips_punct_and_width() {
        assert_eq!(
            norm("连接无线。第三个密码，大写A小写B数字3。"),
            "连接无线第三个密码大写A小写B数字3"
        );
        assert_eq!(norm("  WiFi 　测试 ！"), "WiFi测试");
        assert_eq!(norm("Ａｂ３"), "Ab3");
    }

    // ---------------- 连网：命令优先（2026-09-06 重设计） ----------------

    #[test]
    fn wifi_words_fire_netconnect() {
        for w in ["连接无线网络", "连网", "连wifi", "上网", "连一下"] {
            let mut vm = Vm::new();
            let o = heard(&mut vm, w);
            assert!(acts(&o).contains(&Act::NetConnect), "「{w}」应触发 NetConnect");
            assert!(!acts(&o).contains(&Act::QrScan));
        }
        // 相机词优先：扫码连无线给相机，不给网络
        let mut vm = Vm::new();
        let o = heard(&mut vm, "扫码连无线");
        assert!(acts(&o).contains(&Act::QrScan));
        assert!(!acts(&o).contains(&Act::NetConnect));
    }

    #[test]
    fn netstate_up_says_connected_only() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::NetState(NetState::Up));
        assert_eq!(says(&o), vec!["网已连。"]);
        assert!(acts(&o).is_empty()); // 机器不画蛇添足
    }

    #[test]
    fn netstate_missing_or_failed_opens_eye() {
        for ns in [NetState::NoConf, NetState::ConfFail] {
            let mut vm = Vm::new();
            let o = vm.step(Ev::NetState(ns));
            assert_eq!(says(&o), vec!["对准配对码或无线码。"]);
            assert!(acts(&o).contains(&Act::Eye));
        }
    }

    #[test]
    fn pair_code_auto_applies_without_confirmation() {
        let b = aginx_qr::PairBundle::try_new(
            "Legrand AP",
            "p4ss w0rd!",
            "sk-123",
            "cf49973e",
            "relaysec",
        )
        .unwrap();
        let mut vm = Vm::new();
        let o = vm.step(Ev::QrDone(Ok(vec![b.payload()])));
        assert_eq!(says(&o), vec!["扫到Legrand AP的配对码，连接配对。"]);
        assert_eq!(
            acts(&o),
            vec![Act::PairApply {
                bundle: b.clone()
            }]
        );
        // 秘密卫生：psk / brain key / relay secret 一个都不上对话行
        let shown: String = vm.lines().iter().map(|(_, s)| s.as_str()).collect();
        assert!(!shown.contains("p4ss"));
        assert!(!shown.contains("sk-123"));
        assert!(!shown.contains("relaysec"));
    }

    #[test]
    fn pair_code_beats_wifi_code() {
        let b = aginx_qr::PairBundle::try_new("ap", "pw", "k", "g", "s").unwrap();
        let mut vm = Vm::new();
        let o = vm.step(Ev::QrDone(Ok(vec![
            "WIFI:T:WPA;S:other;P:zz;;".into(),
            b.payload(),
        ])));
        assert!(matches!(acts(&o).as_slice(), &[Act::PairApply { .. }]));
        assert!(!o.iter().any(|x| matches!(x, Out::Act(Act::Join { .. }))));
    }

    #[test]
    fn connect_noconf_then_wifi_code_happy_path() {
        let mut vm = Vm::new();
        let o = heard(&mut vm, "连网");
        assert_eq!(says(&o), vec!["看一下网络。"]);
        assert!(acts(&o).contains(&Act::NetConnect));
        let o = vm.step(Ev::NetState(NetState::NoConf));
        assert!(acts(&o).contains(&Act::Eye));
        // 用户把 WiFi 码怼到镜头前
        let o = vm.step(Ev::QrDone(Ok(vec!["WIFI:T:WPA;S:home-ap;P:secret8;;".into()])));
        assert_eq!(says(&o), vec!["扫到网络home-ap，密码7位，连接。"]);
        assert!(acts(&o).contains(&Act::Join {
            ssid: "home-ap".into(),
            psk: "secret8".into()
        }));
        let shown: String = vm.lines().iter().map(|(_, s)| s.as_str()).collect();
        assert!(!shown.contains("secret8")); // psk 永不上屏
        let o = vm.step(Ev::JoinDone(Ok("192.168.0.166".into())));
        assert_eq!(says(&o), vec!["连上了，地址192.168.0.166。"]);
        assert_eq!(vm.state_name(), "idle");
    }

    #[test]
    fn pair_done_reports_without_reopening_eye() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::PairDone(Ok("网已连，母体在线".into())));
        assert_eq!(says(&o), vec!["配对完成，网已连，母体在线。"]);
        assert_eq!(
            speaks(&o),
            vec!["Connection restored. Welcome to the real world."]
        );
        assert!(acts(&o).is_empty());
        let o = vm.step(Ev::PairDone(Err("时钟没同步".into())));
        assert_eq!(says(&o), vec!["配对没成，时钟没同步。再说连网重试。"]);
        assert!(acts(&o).is_empty()); // 失败不自动循环，人再说连网
    }

    #[test]
    fn cancel_closes_the_eye() {
        let mut vm = Vm::new();
        let o = heard(&mut vm, "算了");
        assert_eq!(says(&o), vec!["已取消。"]);
        assert!(acts(&o).contains(&Act::EyeClose));
    }

    // ---------------- M42b QR ----------------

    #[test]
    fn scan_word_fires_qr_not_wifi() {
        let mut vm = Vm::new();
        let o = heard(&mut vm, "扫码");
        assert!(acts(&o).contains(&Act::QrScan));
        // 纯 wifi 词不碰相机
        let mut vm = Vm::new();
        let o = heard(&mut vm, "连接无线网络");
        assert!(acts(&o).contains(&Act::NetConnect));
        assert!(!acts(&o).contains(&Act::QrScan));
    }

    #[test]
    fn qr_wifi_flow_auto_joins() {
        // M42g：扫码即指令——WIFI: 码不过确认，直接 Act::Join
        let mut vm = Vm::new();
        // 多码：跳过非 WIFI 码取第一个 WIFI
        let o = vm.step(Ev::QrDone(Ok(vec![
            "https://aginx.net".into(),
            "WIFI:T:WPA;S:home-ap;P:secret8;;".into(),
        ])));
        assert_eq!(says(&o), vec!["扫到网络home-ap，密码7位，连接。"]);
        assert!(o.contains(&Out::Act(Act::Join {
            ssid: "home-ap".into(),
            psk: "secret8".into()
        })));
        let o = vm.step(Ev::JoinDone(Ok("10.0.0.5".into())));
        assert_eq!(says(&o), vec!["连上了，地址10.0.0.5。"]);
        assert_eq!(vm.state_name(), "idle");
    }

    #[test]
    fn qr_open_net_auto_joins_empty_psk() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::QrDone(Ok(vec!["WIFI:S:opennet;;".into()])));
        assert_eq!(says(&o), vec!["扫到网络opennet，开放网络，连接。"]);
        assert!(o.contains(&Out::Act(Act::Join {
            ssid: "opennet".into(),
            psk: "".into()
        })));
    }

    #[test]
    fn qr_text_truncated_and_empty_and_err() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::QrDone(Ok(vec!["https://aginx.net/pkg".into()])));
        assert_eq!(says(&o), vec!["扫到，https://aginx.net/pkg。"]);
        // 长文本截到 40 字 + 省略号
        let long = "a".repeat(50);
        let o = vm.step(Ev::QrDone(Ok(vec![long])));
        assert_eq!(says(&o), vec![format!("扫到，{}…", "a".repeat(40))]);
        // 拍了但没码 / 相机失败：同一句引导（原因进 daemon 日志）
        let o = vm.step(Ev::QrDone(Ok(vec![])));
        assert_eq!(says(&o), vec!["没拍到二维码，正对着它再说扫码。"]);
        let o = vm.step(Ev::QrDone(Err("cam-shot rc=1".into())));
        assert_eq!(says(&o), vec!["没拍到二维码，正对着它再说扫码。"]);
        assert_eq!(vm.state_name(), "idle");
    }

    // ---------------- M45 OCR ----------------

    #[test]
    fn ocr_fires_from_idle_and_not_scan() {
        for w in ["念一下", "读一下", "念文字", "读文字", "念给我听", "这是什么字"] {
            let mut vm = Vm::new();
            let o = heard(&mut vm, w);
            assert!(o.contains(&Out::Act(Act::Ocr)), "「{w}」应触发 OCR");
            assert!(!o.contains(&Out::Act(Act::QrScan)), "「{w}」不应触发扫码");
            assert!(!o.contains(&Out::Act(Act::NetConnect)), "「{w}」不应触发连网");
        }
        // 反向：扫码词不进 OCR
        let mut vm = Vm::new();
        let o = heard(&mut vm, "扫码");
        assert!(o.contains(&Out::Act(Act::QrScan)));
        assert!(!o.contains(&Out::Act(Act::Ocr)));
    }

    #[test]
    fn ocr_done_short_reads_all_long_reads_head() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::OcrDone(Ok(vec![
            "机器视觉测试".into(),
            "TEL 138-0013-8000".into(),
        ])));
        // 短文：行拼成一句整念（daemon split_clauses 分句）——拉式语音下
        // 念一下仍是点名面：Speak
        assert_eq!(speaks(&o), vec!["机器视觉测试。TEL 138-0013-8000"]);
        // 全文上屏：每行一条对话行（Speak 不再重复推行）
        let shown: Vec<&str> = vm.lines().iter().map(|(_, s)| s.as_str()).collect();
        assert!(shown.contains(&"机器视觉测试"));
        assert!(shown.contains(&"TEL 138-0013-8000"));
        assert_eq!(vm.lines().len(), 2);

        // 长文（>120 字）：只念前两行 + 指屏
        let mut vm = Vm::new();
        let lines: Vec<String> = (0..5).map(|i| format!("第{i}行{}", "字".repeat(30))).collect();
        let o = vm.step(Ev::OcrDone(Ok(lines.clone())));
        assert_eq!(
            speaks(&o),
            vec![format!("{}。{}。全文在屏幕上。", lines[0], lines[1])]
        );
        assert_eq!(vm.lines().len(), 5);
    }

    #[test]
    fn ocr_done_empty_and_err_prompt_retry() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::OcrDone(Ok(vec![])));
        assert_eq!(says(&o), vec!["没拍到文字，正对着它再说念一下。"]);
        let o = vm.step(Ev::OcrDone(Err("ag-ocr rc=2".into())));
        assert_eq!(says(&o), vec!["没拍到文字，正对着它再说念一下。"]);
    }

    // ---------------- 拉式语音（2026-09-04） ----------------

    #[test]
    fn speak_request_replays_last_agent_line() {
        let mut vm = Vm::new();
        // 没说过话：点名 → 出声告知（而不是无声）
        let o = heard(&mut vm, "你说给我听");
        assert_eq!(speaks(&o), vec!["我还没说过话。"]);
        // 有过话语：复述最近一条机器行
        let o = heard(&mut vm, "你好");
        assert_eq!(says(&o), vec!["我在。说连网，或说扫码、念一下。"]);
        let o = heard(&mut vm, "说给我听");
        assert_eq!(speaks(&o), vec!["我在。说连网，或说扫码、念一下。"]);
        // 连网提示也一样能复述
        let _ = heard(&mut vm, "连网");
        let o = heard(&mut vm, "再念一遍");
        assert_eq!(speaks(&o), vec!["看一下网络。"]);
    }

    #[test]
    fn chatter_is_quiet_by_default() {
        // 拉式语音：常规回应是 Say（daemon 只上脸），不是 Speak
        let mut vm = Vm::new();
        let o = heard(&mut vm, "无线");
        assert!(!o.iter().any(|o| matches!(o, Out::Speak(_))));
        let o = vm.step(Ev::JoinDone(Ok("192.168.0.166".into())));
        assert_eq!(says(&o), vec!["连上了，地址192.168.0.166。"]);
        assert!(o.iter().all(|o| !matches!(o, Out::Speak(_))));
        // 状态查询（inject_say）也是 Say——屏显，想听跟一句你说给我听
        let o = heard(&mut vm, "状态");
        assert!(matches!(o[0], Out::Say(_)));
    }

    #[test]
    fn empty_heard_prompts_retry() {
        // 2026-09-04 改：空 ASR 静默=用户以为死了；真误触(<0.1s)在
        // capture_take 已拦，走到这的都是 ≥0.1s 的真尝试，必须开口。
        let mut vm = Vm::new();
        let o = heard(&mut vm, "。！？ ");
        assert_eq!(says(&o), vec!["没听清。请按住键说。"]);
    }

    #[test]
    fn gibberish_gets_fixed_reply() {
        let mut vm = Vm::new();
        let o = heard(&mut vm, "今天天气哈哈哈");
        assert_eq!(says(&o), vec!["没听懂。说连网，或扫码，或念一下。"]);
    }

    // ---------------- N2②：前台模式（自由文本 → Act::Chat） ----------------

    #[test]
    fn front_off_free_text_falls_to_didnotunderstand() {
        // 老行为分毫不动：不开前台，自由文本还是没听懂地板
        let mut vm = Vm::new();
        let o = heard(&mut vm, "今天北京天气怎么样");
        assert_eq!(says(&o), vec!["没听懂。说连网，或扫码，或念一下。"]);
        assert!(o.iter().all(|x| !matches!(x, Out::Act(Act::Chat(_)))));
    }

    #[test]
    fn front_on_free_text_routes_to_chat() {
        let mut vm = Vm::with_front();
        let o = heard(&mut vm, "今天北京天气怎么样");
        assert_eq!(says(&o), vec!["问母体，稍等。"]);
        let chat = o.iter().find_map(|x| match x {
            Out::Act(Act::Chat(t)) => Some(t.clone()),
            _ => None,
        });
        assert_eq!(chat, Some("今天北京天气怎么样".to_string()));
    }

    #[test]
    fn front_on_closed_vocab_stays_local() {
        // 离线地板不进 brain 往返：连网/状态/问好全本地
        let mut vm = Vm::with_front();
        let o = heard(&mut vm, "连接无线网络");
        assert!(matches!(o.iter().find(|x| matches!(x, Out::Act(_))), Some(Out::Act(Act::NetConnect))));
        assert_eq!(says(&o), vec!["看一下网络。"]);

        let o = heard(&mut vm, "你好");
        assert_eq!(says(&o), vec!["我在。说连网，或说扫码、念一下。"]);

        let o = heard(&mut vm, "现在几点了");
        assert!(matches!(
            o.iter().find(|x| matches!(x, Out::Act(_))),
            Some(Out::Act(Act::Status))
        ));
    }

    #[test]
    fn front_on_cancel_and_speak_still_work() {
        // 取消/点名出声与前台无关：优先级在自由文本之前
        let mut vm = Vm::with_front();
        let o = heard(&mut vm, "取消");
        assert_eq!(says(&o), vec!["已取消。"]);

        let _ = heard(&mut vm, "今天北京天气怎么样"); // 问母体（daemon 出去）
        let o = heard(&mut vm, "你说给我听");
        assert_eq!(speaks(&o).len(), 1); // 复述最后一句机器行
    }
}
