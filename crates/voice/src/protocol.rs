//! 语音对话协议 v0 — 无 LLM 的确定性状态机（M42a，产品定义 2026-09-04）。
//!
//! 手机即智能体：人的输入只有说（ASR 文本）和看（M42b），输出是嘴（TTS）
//! 和脸（屏幕）。本模块把 ASR 文本对封闭词表做确定性解析，驱动
//! 听→选（序数）→拼（字母/数字）→回读确认→执行 的对话流。
//! 不做模糊匹配、不做意图猜测——没听懂就说没听懂，这是自举地板：
//! WiFi 必须在 LLM 可用之前连得上。
//!
//! 纯逻辑、零 I/O：daemon（main.rs）喂 Ev，收 Out；所有副作用（TTS、
//! 扫描、wifi-join）都是 Out::Say / Out::Act，由 daemon 落地。

// ---------------- events / outputs ----------------

#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    /// 一段 ASR 识别文本（已归一化由本模块完成，原样传入即可）
    Heard(String),
    /// Act::Scan 完成，携带去重后的 SSID 列表
    ScanDone(Vec<String>),
    /// Act::Join 完成：Ok(ip) / Err(原因)
    JoinDone(Result<String, String>),
    /// Act::QrScan 完成：Ok(每码一条 payload) / Err(拍照/解码失败原因)
    QrDone(Result<Vec<String>, String>),
    /// Act::Ocr 完成：Ok(每行一条文本) / Err(拍照/识别失败原因)
    OcrDone(Result<Vec<String>, String>),
    /// 提示后超时无语音（daemon 计时喂入）
    Timeout,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// 扫描 Wi-Fi（nlscan，daemon 解析去重后回 ScanDone）
    Scan,
    /// 加入网络（wifi-join wlan0 ssid psk）
    Join { ssid: String, psk: String },
    /// 拍照解 QR（cam-shot 盲拍 + agqr，M42b 眼分支）
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

#[derive(Debug, Clone, PartialEq)]
enum St {
    Idle,
    /// 列表已展示，等序数
    WifiList,
    /// 等密码拼读，逐段累积
    WifiPwd {
        ssid: String,
        psk: String,
    },
    /// 密码已回读，等确认
    WifiConfirm {
        ssid: String,
        psk: String,
    },
}

pub struct Vm {
    st: St,
    /// 对话行（(谁, 文本)），屏显用；cap 8 行滚动
    lines: Vec<(bool, String)>,
    /// 列表内容（SSID），屏显用
    list: Vec<String>,
    /// 已选中的列表项（1-based；0 = 未选）
    sel: usize,
    /// 自由文本改投新前台（N2②）。false = 老行为（没听懂地板）。
    /// new() 恒 false——上千行既有测试不动，开前台一律 with_front()。
    front: bool,
}

impl Vm {
    pub fn new() -> Vm {
        Vm {
            st: St::Idle,
            lines: Vec::new(),
            list: Vec::new(),
            sel: 0,
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
        match self.st {
            St::Idle => "idle",
            St::WifiList => "list",
            St::WifiPwd { .. } => "pwd",
            St::WifiConfirm { .. } => "confirm",
        }
    }

    /// 屏显对话行：(is_user, text)
    pub fn lines(&self) -> &[(bool, String)] {
        &self.lines
    }
    pub fn list(&self) -> &[String] {
        &self.list
    }
    pub fn sel(&self) -> usize {
        self.sel
    }
    /// 当前半成品密码（屏显/回读用）
    pub fn psk(&self) -> &str {
        match &self.st {
            St::WifiPwd { psk, .. } | St::WifiConfirm { psk, .. } => psk,
            _ => "",
        }
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
                    self.say(&mut outs, "没听清。请按住键，说完整的：连接无线网络。");
                    outs.push(Out::Show);
                    return outs;
                }
                self.heard_line(&raw);
                // 取消优先于一切状态
                if is_cancel(&text) {
                    self.st = St::Idle;
                    self.sel = 0;
                    self.say(&mut outs, "已取消。");
                    outs.push(Out::Show);
                    return outs;
                }
                // 点名要听（拉式语音）：复述最近一条机器话语。confirm 态
                // 不拦——那里的「再念一遍」是密码回读（同词状态义不同）。
                if !matches!(self.st, St::WifiConfirm { .. }) && is_speak_request(&text) {
                    match self.lines.iter().rev().find(|(u, _)| !u) {
                        Some((_, s)) => outs.push(Out::Speak(s.clone())),
                        None => self.say_loud(&mut outs, "我还没说过话。"),
                    }
                    outs.push(Out::Show);
                    return outs;
                }
                match std::mem::replace(&mut self.st, St::Idle) {
                    St::Idle => self.step_idle(&text, &mut outs),
                    St::WifiList => self.step_list(&text, &mut outs),
                    St::WifiPwd { ssid, psk } => self.step_pwd(&text, ssid, psk, &mut outs),
                    St::WifiConfirm { ssid, psk } => self.step_confirm(&text, ssid, psk, &mut outs),
                }
            }
            Ev::ScanDone(list) => {
                self.list = list;
                let n = self.list.len();
                if n == 0 {
                    self.st = St::Idle;
                    self.say(&mut outs, "没找到网络。");
                } else {
                    self.st = St::WifiList;
                    self.sel = 0;
                    self.say(&mut outs, &format!("找到{n}个网络，屏幕上选，说选第几个。"));
                }
                outs.push(Out::Show);
            }
            Ev::JoinDone(r) => {
                self.st = St::Idle;
                match r {
                    Ok(ip) => self.say(&mut outs, &format!("连上了，地址{ip}。")),
                    Err(e) => self.say(&mut outs, &format!("没连上，{e}。再试可以说无线。")),
                }
                outs.push(Out::Show);
            }
            Ev::QrDone(r) => {
                // 第一个可解析的 WIFI: 码直入 confirm。QR 是可信输入（码是
                // 别人机器生成的，不是耳朵听来的）——不回读 PSK 全文，只报
                // 位数；全文在屏上（psk() 本来就展示）。
                let wifi = match &r {
                    Ok(payloads) => payloads
                        .iter()
                        .find_map(|p| agqr::parse_wifi_payload(p)),
                    Err(_) => None,
                };
                match wifi {
                    Some(w) => {
                        let ask = if w.psk.is_empty() {
                            format!("扫到网络{}，开放网络，连接吗？", w.ssid)
                        } else {
                            format!(
                                "扫到网络{}，密码{}位，连接吗？",
                                w.ssid,
                                w.psk.chars().count()
                            )
                        };
                        self.st = St::WifiConfirm {
                            ssid: w.ssid,
                            psk: w.psk,
                        };
                        self.say(&mut outs, &ask);
                    }
                    None => {
                        self.st = St::Idle;
                        match r {
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
                        }
                    }
                }
                outs.push(Out::Show);
            }
            Ev::OcrDone(r) => {
                // M45 眼分支：识别行全文上屏（每行一条对话行，滚动窗取尾），
                // 语音走 Say。短文整篇念（daemon 侧 split_clauses 分句流水）；
                // 长文（>120 字）只念前两行——耳朵听不下整页，眼睛看屏。
                self.st = St::Idle;
                match r {
                    Ok(lines) if !lines.is_empty() => {
                        for l in &lines {
                            self.lines.push((false, l.clone()));
                        }
                        self.trim_lines();
                        // 念一下本身就是要听（拉式语音的点名面）：全文已在
                        // 屏上，Speak 只出声不再推行。
                        let joined = lines.join("。");
                        if joined.chars().count() > 120 {
                            let head = lines.iter().take(2).cloned().collect::<Vec<_>>().join("。");
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
            Ev::Timeout => {
                if !matches!(self.st, St::Idle) {
                    self.st = St::Idle;
                    self.sel = 0;
                    self.say(&mut outs, "超时，已退出。");
                    outs.push(Out::Show);
                }
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
            // 念读也判在 is_wifi 前（「念一���」类词不含网络词，纯判序保守）
            self.say(outs, "拍照念字，对准文字别动。");
            outs.push(Out::Act(Act::Ocr));
        } else if is_wifi(text) {
            self.say(outs, "扫描网络。");
            outs.push(Out::Act(Act::Scan));
        } else if is_status(text) {
            self.say(outs, "看一下。");
            outs.push(Out::Act(Act::Status));
        } else if is_hello(text) {
            self.say(outs, "我在。说连接无线网络，或说扫码、念一下。");
        } else if is_help(text) {
            self.say(
                outs,
                "我能连无线、扫码、念字。回应都在屏幕上，要听我说，说你说给我听。",
            );
        } else {
            if self.front {
                // N2②：封闭词表 miss = 自由文本 → 母体（新前台）。封闭
                // 词表本身仍本地优先（WiFi/扫码/念读是离线地板，不进
                // brain 往返）；前台不可达由 daemon 收兜底话，这里不降级。
                self.say(outs, "问母体，稍等。");
                outs.push(Out::Act(Act::Chat(text.to_string())));
            } else {
                self.say(outs, "没听懂。说完整的：连接无线网络，或扫码，或念一下。");
            }
        }
        outs.push(Out::Show);
    }

    fn step_list(&mut self, text: &str, outs: &mut Vec<Out>) {
        if let Some(n) = match_ordinal(text) {
            if n >= 1 && n as usize <= self.list.len() {
                let ssid = self.list[(n - 1) as usize].clone();
                self.sel = n as usize;
                self.st = St::WifiPwd {
                    ssid: ssid.clone(),
                    psk: String::new(),
                };
                self.say(
                    outs,
                    &format!(
                        "第{n}个，{ssid}。说密码，字母说大写A小写b，数字说数字三。说完说完了。"
                    ),
                );
            } else {
                self.st = St::WifiList;
                self.say(outs, &format!("只有{}个，重新说。", self.list.len()));
            }
        } else if is_scan(text) {
            // 列表态说扫码：弃列表转相机（QrDone 会重置状态）
            self.st = St::Idle;
            self.say(outs, "拍照扫码，对准二维码别动。");
            outs.push(Out::Act(Act::QrScan));
        } else if is_ocr(text) {
            // 列表态说念读：弃列表转相机（OcrDone 会重置状态）
            self.st = St::Idle;
            self.say(outs, "拍照念字，对准文字别动。");
            outs.push(Out::Act(Act::Ocr));
        } else if is_wifi(text) || is_rescan(text) {
            self.st = St::WifiList;
            self.say(outs, "重新扫描。");
            outs.push(Out::Act(Act::Scan));
        } else {
            self.st = St::WifiList;
            self.say(outs, "说选第几个，或者说取消退出。");
        }
        outs.push(Out::Show);
    }

    fn step_pwd(&mut self, text: &str, ssid: String, mut psk: String, outs: &mut Vec<Out>) {
        if is_done(text) {
            if psk.is_empty() {
                self.say(outs, "还没听到密码，继续说。");
                self.st = St::WifiPwd { ssid, psk };
            } else {
                let readback = spell_readback(&psk);
                self.say(outs, &format!("密码是{readback}，对吗？"));
                self.st = St::WifiConfirm { ssid, psk };
            }
        } else if is_backspace(text) {
            psk.pop();
            let readback = spell_readback(&psk);
            self.say(outs, &format!("删掉一个，{readback}。"));
            self.st = St::WifiPwd { ssid, psk };
        } else if is_restart(text) {
            self.say(outs, "重新说密码。");
            self.st = St::WifiPwd {
                ssid,
                psk: String::new(),
            };
        } else {
            let chars = parse_spelling(text);
            if chars.is_empty() {
                self.say(outs, "没听清。字母说大写A，数字说数字三。");
                self.st = St::WifiPwd { ssid, psk };
            } else {
                psk.extend(chars.iter().cloned());
                let readback = spell_readback(&psk);
                self.say(outs, &format!("收到，{readback}。继续，或说完了。"));
                self.st = St::WifiPwd { ssid, psk };
            }
        }
        outs.push(Out::Show);
    }

    fn step_confirm(&mut self, text: &str, ssid: String, psk: String, outs: &mut Vec<Out>) {
        if is_deny(text) {
            self.st = St::WifiPwd {
                ssid,
                psk: String::new(),
            };
            self.say(outs, "重新说密码。");
        } else if is_confirm(text) {
            self.st = St::Idle;
            self.sel = 0;
            self.say(outs, "连接中。");
            outs.push(Out::Act(Act::Join { ssid, psk }));
        } else if is_readback(text) {
            let readback = spell_readback(&psk);
            self.st = St::WifiConfirm { ssid, psk };
            // 回读是点名要听：出声
            self.say_loud(outs, &format!("密码是{readback}，对吗？"));
        } else {
            self.st = St::WifiConfirm { ssid, psk };
            self.say(outs, "说对，或说不对。");
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

fn is_confirm(t: &str) -> bool {
    contains_any(
        t,
        &[
            "对", "是的", "确认", "没错", "好的", "好", "是", "正确", "可以",
        ],
    )
}

fn is_deny(t: &str) -> bool {
    // 否定必须先于肯定判（"不对" 含 "对"）
    contains_any(
        t,
        &[
            "不对",
            "不是",
            "错了",
            "错误",
            "否",
            "不行",
            "不可以",
            "换一个",
        ],
    )
}

fn is_done(t: &str) -> bool {
    contains_any(
        t,
        &[
            "完了",
            "说完了",
            "好了",
            "结束",
            "没有了",
            "没了",
            "搞定",
            "完毕",
        ],
    )
}

fn is_backspace(t: &str) -> bool {
    contains_any(t, &["删掉", "退格", "删一个", "删除"])
}

fn is_restart(t: &str) -> bool {
    contains_any(t, &["重说", "重来", "重新说", "重新来"])
}

fn is_rescan(t: &str) -> bool {
    contains_any(t, &["刷新", "重新扫", "再扫"])
}

fn is_readback(t: &str) -> bool {
    contains_any(t, &["再说一遍", "重复", "念一遍", "再念"])
}

/// 点名要听（拉式语音的总开关词）。超集于 is_readback——confirm 态之外的
/// 任何状态说这些词，都复述最近一条机器话语。「念给我听」不收：那是
/// OCR 的拍+念（is_ocr），同词不同义。
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
    // 相机主动词。与 is_rescan（刷新/重新扫/再扫）不撞：那些词不含
    // 「扫码」「扫一下」——「再扫一下」会进相机，语义上也没错（用户想再拍）。
    contains_any(t, &["扫码", "二维码", "扫一扫", "扫一下", "扫个码"])
}

fn is_ocr(t: &str) -> bool {
    // M45 念读主动词。与 is_readback（念一遍/再说一遍——confirm 态专用）
    // 不撞：「念一下」≠「念一遍」。裸「念」「读」不收（「念念不忘」误触）。
    contains_any(
        t,
        &["念一下", "读一下", "念文字", "读文字", "念给我听", "这是什么字"],
    )
}

// ---------------- 序数 ----------------

/// 中文数字词 → 数值（v0 支持一..十 + ASCII 数字 + 组合 十一..九十九 不做，
/// 列表通常 ≤10）。返回 None 表示不是数字词。
fn digit_val(s: &str) -> Option<u8> {
    match s {
        "零" | "〇" | "0" => Some(0),
        "一" | "1" => Some(1),
        "二" | "两" | "2" => Some(2),
        "三" | "3" => Some(3),
        "四" | "4" => Some(4),
        "五" | "5" => Some(5),
        "六" | "6" => Some(6),
        "七" | "7" => Some(7),
        "八" | "8" => Some(8),
        "九" | "9" => Some(9),
        "十" | "10" => Some(10),
        _ => None,
    }
}

/// 从归一化文本里解析序数选择："第三个" / "3个" / 整句就是 "三" / "第2个吧"。
/// 扫描顺序：第X个 → X个 → 裸 X。
pub fn match_ordinal(t: &str) -> Option<u8> {
    // 第X个
    if let Some(i) = t.find('第') {
        let rest = &t[i + '第'.len_utf8()..];
        for len in 1..=2.min(rest.chars().count()) {
            let head: String = rest.chars().take(len).collect();
            if let Some(v) = digit_val(&head) {
                // 后面跟 个 即命中（"第三个" / "第3个吧"）
                let after: String = rest.chars().skip(len).take(1).collect();
                if after == "个" {
                    return Some(v);
                }
            }
        }
    }
    // X个（整串开头）
    let mut chars = t.chars();
    if let Some(first) = chars.next() {
        let one = first.to_string();
        if let Some(v) = digit_val(&one) {
            let second: String = chars.take(1).collect();
            if second == "个" {
                return Some(v);
            }
        }
    }
    // 裸数字词（整句就是一个数字——列表态最常见："三"）
    if t.chars().count() <= 2 {
        let mut parts = t.chars();
        let one: String = parts.next().map(String::from).unwrap_or_default();
        let two: String = parts.next().map(String::from).unwrap_or_default();
        if let Some(v) = digit_val(&one) {
            // "3" / "三"；"10" 是两个字符 '1'+'0'
            if two.is_empty() {
                return Some(v);
            }
        }
        if t == "10" {
            return Some(10);
        }
    }
    None
}

// ---------------- 拼读 ----------------

/// 从归一化文本提取拼读字符序列：大写A→'A'，小写b→'b'，数字3→'3'，
/// 裸字母/数字按原样收。未知片段跳过（ASR 噪声）。
pub fn parse_spelling(t: &str) -> Vec<char> {
    let chars: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        // 大写/小写 + 字母
        for (word, upper) in [("大写", true), ("小写", false)] {
            if rest.starts_with(word) {
                if let Some(&c) = chars.get(i + word.chars().count()) {
                    if c.is_ascii_alphabetic() {
                        out.push(if upper {
                            c.to_ascii_uppercase()
                        } else {
                            c.to_ascii_lowercase()
                        });
                        i += word.chars().count() + 1;
                        continue 'outer;
                    }
                }
            }
        }
        // 数字 + 数字词
        if rest.starts_with("数字") {
            let one: String = chars.get(i + 2).map(|c| c.to_string()).unwrap_or_default();
            let two: String = chars.get(i + 3).map(|c| c.to_string()).unwrap_or_default();
            if let Some(v) = digit_val(&one) {
                out.push((b'0' + v) as char);
                i += 3;
                continue;
            }
            let pair = format!("{one}{two}");
            if pair == "10" {
                // "数字一零" 连读（罕见），v0 不支持，跳过
            }
        }
        let c = chars[i];
        if c.is_ascii_alphanumeric() {
            out.push(c);
            i += 1;
            continue;
        }
        // 裸数字词（"数字一零"里的零、口语溜掉的"七"）也收——密码态整句
        // 就是拼读，词表外的汉字走 skip，数字词不该丢
        let one = c.to_string();
        if let Some(v) = digit_val(&one) {
            out.push((b'0' + v) as char);
            i += 1;
            continue;
        }
        i += 1; // 噪声词（"那个""呃"等）跳过
    }
    out
}

/// 回读串：密码字符 → 可 TTS 的拼读文本。"Ab3" → "大写A，小写b，数字3"
pub fn spell_readback(psk: &str) -> String {
    psk.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                format!("大写{c}")
            } else if c.is_ascii_lowercase() {
                format!("小写{c}")
            } else {
                format!("数字{c}")
            }
        })
        .collect::<Vec<_>>()
        .join("，")
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

    #[test]
    fn norm_strips_punct_and_width() {
        assert_eq!(
            norm("连接无线。第三个密码，大写A小写B数字3。"),
            "连接无线第三个密码大写A小写B数字3"
        );
        assert_eq!(norm("  WiFi 　测试 ！"), "WiFi测试");
        assert_eq!(norm("Ａｂ３"), "Ab3");
    }

    #[test]
    fn ordinal_forms() {
        assert_eq!(match_ordinal("第三个"), Some(3));
        assert_eq!(match_ordinal("第3个吧"), Some(3));
        assert_eq!(match_ordinal("第十个"), Some(10));
        assert_eq!(match_ordinal("就第二个"), Some(2));
        assert_eq!(match_ordinal("5个"), Some(5));
        assert_eq!(match_ordinal("三"), Some(3));
        assert_eq!(match_ordinal("10"), Some(10));
        assert_eq!(match_ordinal("俩"), None);
        assert_eq!(match_ordinal("无线"), None);
    }

    #[test]
    fn spelling_forms() {
        assert_eq!(parse_spelling("大写A小写b数字3"), vec!['A', 'b', '3']);
        // ASR 常把字母全大写呈现：小写B 仍是小写语义（大小写来自说的前缀）
        assert_eq!(parse_spelling("大写A小写B数字3"), vec!['A', 'b', '3']);
        assert_eq!(parse_spelling("大写ABC"), vec!['A', 'B', 'C']); // 连念大写默认全大
        assert_eq!(parse_spelling("aB9"), vec!['a', 'B', '9']);
        assert_eq!(parse_spelling("呃数字七那个小写m"), vec!['7', 'm']);
        assert_eq!(parse_spelling("数字三数字一零"), vec!['3', '1', '0']);
    }

    #[test]
    fn readback_roundtrip() {
        assert_eq!(spell_readback("Ab3"), "大写A，小写b，数字3");
        // 拼读→回读可循环：人听到的格式 == 说出的格式
        let chars = parse_spelling("大写A小写b数字3");
        let s: String = chars.into_iter().collect();
        assert_eq!(spell_readback(&s), "大写A，小写b，数字3");
    }

    #[test]
    fn confirm_beats_deny_order() {
        assert!(is_deny("不对"));
        assert!(is_confirm("对"));
        assert!(is_confirm("是的没错"));
        // "不对" 判定路径：deny 先查
        assert!(!is_confirm_checked_after_deny("不对"));
    }
    fn is_confirm_checked_after_deny(t: &str) -> bool {
        !is_deny(t) && is_confirm(t)
    }

    #[test]
    fn wifi_flow_full() {
        let mut vm = Vm::new();
        // 1. 触发扫描
        let o = heard(&mut vm, "连接无线。");
        assert!(o.contains(&Out::Act(Act::Scan)));
        // 2. 扫描结果回来
        let o = vm.step(Ev::ScanDone(vec!["Legrand AP".into(), "2501".into()]));
        assert_eq!(says(&o), vec!["找到2个网络，屏幕上选，说选第几个。"]);
        assert_eq!(vm.list(), &["Legrand AP".to_string(), "2501".to_string()]);
        // 3. 序数选择
        let o = heard(&mut vm, "第二个");
        assert_eq!(
            says(&o),
            vec!["第2个，2501。说密码，字母说大写A小写b，数字说数字三。说完说完了。"]
        );
        // 4. 密码拼读（ASR 原文带标点）
        let o = heard(&mut vm, "大写A小写B数字3");
        assert_eq!(says(&o)[0], "收到，大写A，小写b，数字3。继续，或说完了。");
        assert_eq!(vm.psk(), "Ab3"); // 小写B：大小写来自说的前缀
                                     // 5. 完成 → 回读
        let o = heard(&mut vm, "完了");
        assert_eq!(says(&o), vec!["密码是大写A，小写b，数字3，对吗？"]);
        // 6. 确认 → 执行
        let o = heard(&mut vm, "对");
        assert!(o.contains(&Out::Act(Act::Join {
            ssid: "2501".into(),
            psk: "Ab3".into()
        })));
        // 7. 结果
        let o = vm.step(Ev::JoinDone(Ok("192.168.0.166".into())));
        assert_eq!(says(&o), vec!["连上了，地址192.168.0.166。"]);
        assert_eq!(vm.state_name(), "idle");
    }

    #[test]
    fn pwd_backspace_and_restart() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["X".into()]));
        vm.step(Ev::Heard("第一个".into()));
        heard(&mut vm, "大写A数字2");
        assert_eq!(vm.psk(), "A2");
        let _o = heard(&mut vm, "删掉");
        assert_eq!(vm.psk(), "A");
        let _o = heard(&mut vm, "重说");
        assert_eq!(vm.psk(), "");
    }

    #[test]
    fn confirm_deny_reasks_pwd() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("连wifi".into()));
        vm.step(Ev::ScanDone(vec!["S".into()]));
        vm.step(Ev::Heard("第一个".into()));
        heard(&mut vm, "小写z");
        heard(&mut vm, "好了");
        assert_eq!(vm.state_name(), "confirm");
        let _o = heard(&mut vm, "不对");
        assert_eq!(vm.state_name(), "pwd");
        assert_eq!(vm.psk(), "");
    }

    #[test]
    fn ordinal_out_of_range_and_rescan() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("上网".into()));
        vm.step(Ev::ScanDone(vec!["A".into(), "B".into()]));
        let o = heard(&mut vm, "第五个");
        assert_eq!(says(&o), vec!["只有2个，重新说。"]);
        let o = heard(&mut vm, "刷新");
        assert!(o.contains(&Out::Act(Act::Scan)));
    }

    #[test]
    fn cancel_from_any_state() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["A".into(), "B".into()]));
        vm.step(Ev::Heard("第一个".into()));
        heard(&mut vm, "大写Q");
        let o = heard(&mut vm, "算了");
        assert_eq!(says(&o), vec!["已取消。"]);
        assert_eq!(vm.state_name(), "idle");
        assert_eq!(vm.psk(), "");
    }

    #[test]
    fn timeout_resets_and_idle_stays_quiet() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["A".into()]));
        let o = vm.step(Ev::Timeout);
        assert_eq!(says(&o), vec!["超时，已退出。"]);
        let o = vm.step(Ev::Timeout); // Idle 下超时不出声
        assert!(o.is_empty());
    }

    #[test]
    fn gibberish_gets_fixed_reply() {
        let mut vm = Vm::new();
        let o = heard(&mut vm, "今天天气哈哈哈");
        assert_eq!(says(&o), vec!["没听懂。说完整的：连接无线网络，或扫码，或念一下。"]);
    }

    #[test]
    fn empty_heard_prompts_retry() {
        // 2026-09-04 改：空 ASR 静默=用户以为死了；真误触(<0.1s)在
        // capture_take 已拦，走到这的都是 ≥0.1s 的真尝试，必须开口。
        let mut vm = Vm::new();
        let o = heard(&mut vm, "。！？ ");
        assert_eq!(says(&o), vec!["没听清。请按住键，说完整的：连接无线网络。"]);
    }

    #[test]
    fn readback_repeat_in_confirm() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["S".into()]));
        vm.step(Ev::Heard("第一个".into()));
        heard(&mut vm, "大写A");
        heard(&mut vm, "完了");
        let o = heard(&mut vm, "再说一遍");
        // confirm 态的念一遍 = 密码回读（点名，出声）
        assert_eq!(speaks(&o), vec!["密码是大写A，对吗？"]);
    }

    // ---------------- 拉式语音（2026-09-04） ----------------

    #[test]
    fn speak_request_replays_last_agent_line() {
        let mut vm = Vm::new();
        // 没说过话：点名 → 出声告知（而不是无声）
        let o = heard(&mut vm, "你说给我听");
        assert_eq!(speaks(&o), vec!["我还没说过话。"]);
        // 有过话语：复述最近一条机器行，不改状态
        let o = heard(&mut vm, "你好");
        assert_eq!(says(&o), vec!["我在。说连接无线网络，或说扫码、念一下。"]);
        let o = heard(&mut vm, "说给我听");
        assert_eq!(speaks(&o), vec!["我在。说连接无线网络，或说扫码、念一下。"]);
        assert_eq!(vm.state_name(), "idle");
        // 列表态也不拦（复述，列表保持）
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["A".into()]));
        let o = heard(&mut vm, "再念一遍");
        assert_eq!(speaks(&o), vec!["找到1个网络，屏幕上选，说选第几个。"]);
        assert_eq!(vm.state_name(), "list");
        assert_eq!(vm.list(), &["A".to_string()]);
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
    fn asr_real_world_samples() {
        // 2026-09-04 实测 brain ASR 中文样本的归一化路径
        assert_eq!(
            norm("连接无线。第三个密码，大写A小写B数字3。"),
            "连接无线第三个密码大写A小写B数字3"
        );
        // 序数识别在真实 ASR 里常出 "第3个"（阿拉伯数字）
        assert_eq!(match_ordinal(&norm("第3个")), Some(3));
        // 密码段识别常夹标点
        assert_eq!(
            parse_spelling(&norm("密码，大写A小写B数字3。")),
            vec!['A', 'b', '3']
        );
    }

    // ---------------- M42b QR ----------------

    #[test]
    fn scan_fires_qr_before_wifi() {
        let mut vm = Vm::new();
        let o = heard(&mut vm, "扫码");
        assert!(o.contains(&Out::Act(Act::QrScan)));
        // 「扫码连无线」主动词是相机——is_scan 前置于 is_wifi
        let mut vm = Vm::new();
        let o = heard(&mut vm, "扫码连无线");
        assert!(o.contains(&Out::Act(Act::QrScan)));
        assert!(!o.contains(&Out::Act(Act::Scan)));
        // 纯 wifi 词仍走网络扫描，不碰相机
        let mut vm = Vm::new();
        let o = heard(&mut vm, "连接无线网络");
        assert!(o.contains(&Out::Act(Act::Scan)));
        assert!(!o.contains(&Out::Act(Act::QrScan)));
    }

    #[test]
    fn scan_from_list_and_rescan_unaffected() {
        // 列表态说扫码：弃列表转相机
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["A".into()]));
        let o = heard(&mut vm, "扫一下");
        assert!(o.contains(&Out::Act(Act::QrScan)));
        // 列表态重扫词仍是 wifi 扫描
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["A".into()]));
        let o = heard(&mut vm, "刷新");
        assert!(o.contains(&Out::Act(Act::Scan)));
        assert!(!o.contains(&Out::Act(Act::QrScan)));
    }

    #[test]
    fn qr_wifi_flow_to_join() {
        let mut vm = Vm::new();
        // 多码：跳过非 WIFI 码取第一个 WIFI
        let o = vm.step(Ev::QrDone(Ok(vec![
            "https://aginx.net".into(),
            "WIFI:T:WPA;S:home-ap;P:secret8;;".into(),
        ])));
        assert_eq!(says(&o), vec!["扫到网络home-ap，密码7位，连接吗？"]);
        assert_eq!(vm.state_name(), "confirm");
        assert_eq!(vm.psk(), "secret8"); // 屏显全文（语音只报位数）
        let o = heard(&mut vm, "对");
        assert!(o.contains(&Out::Act(Act::Join {
            ssid: "home-ap".into(),
            psk: "secret8".into()
        })));
        let o = vm.step(Ev::JoinDone(Ok("10.0.0.5".into())));
        assert_eq!(says(&o), vec!["连上了，地址10.0.0.5。"]);
        assert_eq!(vm.state_name(), "idle");
    }

    #[test]
    fn qr_open_net_and_deny_to_spell() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::QrDone(Ok(vec!["WIFI:S:opennet;;".into()])));
        assert_eq!(says(&o), vec!["扫到网络opennet，开放网络，连接吗？"]);
        // deny 后 QR 给的 ssid 还在：退到口头拼密码（WPA 网络里开放码可能是
        // 码旧了/家里加过密码）
        let o = heard(&mut vm, "不对");
        assert_eq!(says(&o), vec!["重新说密码。"]);
        assert_eq!(vm.state_name(), "pwd");
        heard(&mut vm, "大写X数字9");
        assert_eq!(vm.psk(), "X9");
    }

    #[test]
    fn qr_text_truncated_and_empty_and_err() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::QrDone(Ok(vec!["https://aginx.net/pkg".into()])));
        assert_eq!(says(&o), vec!["扫到，https://aginx.net/pkg。"]);
        assert_eq!(vm.state_name(), "idle");
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

    #[test]
    fn qr_cancel_during_confirm() {
        // confirm 是既有面：扫码进 confirm 后取消照常退 idle
        let mut vm = Vm::new();
        vm.step(Ev::QrDone(Ok(vec!["WIFI:S:x;P:y;;".into()])));
        let o = heard(&mut vm, "算了");
        assert_eq!(says(&o), vec!["已取消。"]);
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
            assert!(!o.contains(&Out::Act(Act::Scan)), "「{w}」不应触发 wifi 扫描");
        }
        // 反向：扫码词不进 OCR
        let mut vm = Vm::new();
        let o = heard(&mut vm, "扫码");
        assert!(o.contains(&Out::Act(Act::QrScan)));
        assert!(!o.contains(&Out::Act(Act::Ocr)));
    }

    #[test]
    fn ocr_from_list_state_drops_list() {
        let mut vm = Vm::new();
        vm.step(Ev::Heard("无线".into()));
        vm.step(Ev::ScanDone(vec!["A".into()]));
        let o = heard(&mut vm, "念一下");
        assert!(o.contains(&Out::Act(Act::Ocr)));
        assert_eq!(vm.state_name(), "idle"); // OcrDone 会重置，先退出列表态
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
        assert_eq!(
            speaks(&o),
            vec!["机器视觉测试。TEL 138-0013-8000"]
        );
        assert_eq!(vm.state_name(), "idle");
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
        // 屏上五行全文（语音转述不再占对话行）
        assert_eq!(vm.lines().len(), 5);
    }

    #[test]
    fn ocr_done_empty_and_err_prompt_retry() {
        let mut vm = Vm::new();
        let o = vm.step(Ev::OcrDone(Ok(vec![])));
        assert_eq!(says(&o), vec!["没拍到文字，正对着它再说念一下。"]);
        assert_eq!(vm.state_name(), "idle");
        let o = vm.step(Ev::OcrDone(Err("ag-ocr rc=2".into())));
        assert_eq!(says(&o), vec!["没拍到文字，正对着它再说念一下。"]);
    }

    // ---------------- N2②：前台模式（自由文本 → Act::Chat） ----------------

    #[test]
    fn front_off_free_text_falls_to_didnotunderstand() {
        // 老行为分毫不动：不开前台，自由文本还是没听懂地板
        let mut vm = Vm::new();
        let o = heard(&mut vm, "今天北京天气怎么样");
        assert_eq!(says(&o), vec!["没听懂。说完整的：连接无线网络，或扫码，或念一下。"]);
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
        assert_eq!(vm.state_name(), "idle"); // 不占对话状态
    }

    #[test]
    fn front_on_closed_vocab_stays_local() {
        // 离线地板不进 brain 往返：WiFi/状态/问好全本地
        let mut vm = Vm::with_front();
        let o = heard(&mut vm, "连接无线网络");
        assert!(matches!(o.iter().find(|x| matches!(x, Out::Act(_))), Some(Out::Act(Act::Scan))));
        assert_eq!(says(&o), vec!["扫描网络。"]);

        let o = heard(&mut vm, "你好");
        assert_eq!(says(&o), vec!["我在。说连接无线网络，或说扫码、念一下。"]);

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
