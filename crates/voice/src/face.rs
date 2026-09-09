//! 屏显对话面（M42a）：aginx-voice 是 /run/aginx-voice/face 的唯一写者，原子换名写；
//! aterm 轮询 mtime 读（同 /run/aginx-term.inject 的先例）。触摸屏=纯显示器，
//! 脸不归 aginx-voice 画、也不归 aginx-voice 点——没有可点的东西。
//!
//! M42g 眼取景：音量+ 按下 → eye=true 且逐帧换写 eye.jpg（同目录同原子法），
//! term 主区从对话行切为渲染这张帧（成果区第一实例）——取景画面本身就是
//! 「眼睛睁开了」，人看着画面瞄准，识别成功自动进对话。

use crate::protocol::Vm;
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

pub const FACE_DIR: &str = "/run/aginx-voice";
pub const FACE_FILE: &str = "/run/aginx-voice/face";
/// 眼取景当前帧（M42g）。voice 原子换名写，term 轮询 mtime 重渲染。
pub const EYE_JPG: &str = "/run/aginx-voice/eye.jpg";

// 开机剧情 v4：line = 打字文本（ASR transcript v4③ / 文本降级回复，term
// 本地 ~90ms/char 打字机，换串即换行）。剧场字段 call/lines 已随 v4 退役。
static BOOT_LINE: Mutex<Option<String>> = Mutex::new(None);

/// 结果页站立中（v4⑥：真人不看日志——结果页不超时是产品线）。唯一置位
/// 点 write_doc(result=true)，任何 result=false 落盘即清。run_outs 尾部的
/// 例行刷脸凭它跳过——否则 tick 超时/纯 say 再入 run_outs 会把站立页踩回
/// 光标面（真人实测第二雷：live 几秒后必 teardown）。
static RESULT_STANDING: Mutex<bool> = Mutex::new(false);

/// 换打字文本。前缀延长由 term 识别并续打，其余换串从头打。
pub fn set_line(line: Option<&str>) {
    *BOOT_LINE.lock().unwrap() = line.map(str::to_string);
}

/// 打字文本当前值（#282 开机等网行所有权判据）：写入方读回自己是否还
/// 在台上——用户一说话 transcript 就顶掉等待行，watch 即知让位。
pub fn current_line() -> Option<String> {
    BOOT_LINE.lock().unwrap().clone()
}

#[derive(Serialize)]
pub struct FaceDoc<'a> {
    /// 协议状态名（无驻留态状态机恒 "idle"；m42c 钉 "state":"idle"）
    pub state: &'a str,
    /// 眼取景中：Mode::Eye 整屏取景
    pub eye: bool,
    /// 开机剧情 v4 结果面（#246④）：result.img 已就位，term 整屏上帧
    pub result: bool,
    /// 打字文本（transcript/文本回复，'\n' 强制换行）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    pub hint: &'a str,
}

const HINT: &str = "按住音量下说话 · 音量+对码";

fn write_doc(state: &str, eye: bool, result: bool) {
    *RESULT_STANDING.lock().unwrap() = result;
    let _ = fs::create_dir_all(FACE_DIR);
    let doc = FaceDoc {
        state,
        eye,
        result,
        line: BOOT_LINE.lock().unwrap().clone(),
        hint: HINT,
    };
    let tmp = format!("{FACE_FILE}.tmp");
    if let Ok(json) = serde_json::to_vec(&doc) {
        if fs::write(&tmp, &json).is_ok() {
            let _ = fs::rename(&tmp, FACE_FILE);
        }
    }
}

pub fn write(vm: &Vm, eye: bool) {
    write_doc(vm.state_name(), eye, false);
}

/// 结果面（v4④）：render 线程发布 result.img 后调用——state 沿用分���时
/// 捕获的名字，其余全静，result=true 让 term 从光标面切整屏帧。
pub fn write_result(state: &str) {
    write_doc(state, false, true);
}

/// 结果页站立中（v4⑥）：run_outs 尾部例行刷脸的门。
pub fn result_standing() -> bool {
    *RESULT_STANDING.lock().unwrap()
}

/// aginx-term/调试读面
pub fn read() -> Option<String> {
    fs::read_to_string(Path::new(FACE_FILE)).ok()
}
