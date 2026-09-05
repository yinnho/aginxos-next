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

pub const FACE_DIR: &str = "/run/aginx-voice";
pub const FACE_FILE: &str = "/run/aginx-voice/face";
/// 眼取景当前帧（M42g）。voice 原子换名写，term 轮询 mtime 重渲染。
pub const EYE_JPG: &str = "/run/aginx-voice/eye.jpg";

#[derive(Serialize)]
pub struct FaceDoc<'a> {
    /// 协议状态名（无驻留态状态机恒 "idle"；留作未来 Choice 等状态的缝）
    pub state: &'a str,
    /// PTT 按住采集中
    pub listening: bool,
    /// ASR/TTS/执行中
    pub busy: bool,
    /// 眼取景中：term 主区渲染 eye.jpg，对话行退居底部
    pub eye: bool,
    /// 对话行：true=用户说的，false=化身说的
    pub lines: Vec<(bool, String)>,
    pub hint: &'a str,
}

const HINT: &str = "按住音量下说话 · 音量+对码";

pub fn write(vm: &Vm, listening: bool, busy: bool, eye: bool) {
    let _ = fs::create_dir_all(FACE_DIR);
    let doc = FaceDoc {
        state: vm.state_name(),
        listening,
        busy,
        eye,
        lines: vm.lines().to_vec(),
        hint: HINT,
    };
    let tmp = format!("{FACE_FILE}.tmp");
    if let Ok(json) = serde_json::to_vec(&doc) {
        if fs::write(&tmp, &json).is_ok() {
            let _ = fs::rename(&tmp, FACE_FILE);
        }
    }
}

/// aginx-term/调试读面
pub fn read() -> Option<String> {
    fs::read_to_string(Path::new(FACE_FILE)).ok()
}
