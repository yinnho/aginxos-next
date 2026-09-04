// ledger — D8 会话账：append-only JSONL，一行一帧，server 是唯一记账人。
//
// 记账顺序是铁律：**先记账、再 spawn**（「模型可见即已记录」）。runtime
// 的冷恢复重放这份账（avatar::replay_session），重放已含本轮 request，
// runtime 侧有 trailing_request_logged 防叠份。
//
// 每一轮必须以 done 收口：runtime 崩在半路（EOF 无 done���时 server 补一
// 帧 synthetic done(err)，让重放永远落在合法形状上；更深的崩溃残骸
// （悬空 tool_call）由 runtime 的 repair 自愈。

use agi::Frame;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 会话账路径：workspaces/{化身}/sessions/{会话}.jsonl
pub fn session_log(workspaces_root: &Path, avatar: &str, session: &str) -> PathBuf {
    workspaces_root.join(avatar).join("sessions").join(format!("{session}.jsonl"))
}

/// 追加一帧。目录不在就建（新化身首轮）。
pub fn append(log: &Path, frame: &Frame) -> io::Result<()> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(log)?;
    f.write_all(agi::encode(frame).as_bytes())?;
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agi::{Done, Request};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aginx-server-test-ledger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn appends_frames_as_lines_and_creates_dirs() {
        let d = tmp("append");
        let log = session_log(&d, "小满", "main");
        append(
            &log,
            &Frame::Request(Request { avatar: "小满".into(), session: "main".into(), text: "你好".into() }),
        )
        .unwrap();
        append(&log, &Frame::Done(Done::ok("好"))).unwrap();
        let lines: Vec<String> =
            std::fs::read_to_string(&log).unwrap().lines().map(String::from).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""t":"request""#));
        assert!(lines[1].contains(r#""t":"done""#));
    }
}
