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
use std::io::{self, BufRead, BufReader, Write};
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

// ---------------- ③ 回合号 + 补账 ----------------
//
// 回合编号：request 带 turn（从 1 起，每轮 +1），done 回填同号；旧账
// 无 turn 字段（serde 读 0/None），next_turn 按帧计数续号，部署序自由。
//
// 补账（spawn 前跑）：更深的崩溃残骸（悬空 tool_call、未收口 request）
// 由 server 落账清偿——修复一次性、追加式、可审计，不重写历史。借
// dsh 的话：修复落在语义层且持久化，盘上日志永远合法形状。runtime 的
// 内存 repair 降级为最后防线保留。

/// 一次账形扫描的结果：下一回合号 + 残骸清单。
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    /// 下一回合号（账内 request 计数与所见最大 turn 取大 +1；旧账自动续号）
    pub next_turn: u64,
    /// 有 request 无 done 的轮号（该轮未收口——server 自身死在半路的残骸）
    pub open_turn: Option<u64>,
    /// 悬空 tool_call id（全史累计，按打开序；含未收口轮的）
    pub dangling: Vec<String>,
}

/// 扫账：数回合、找残骸。文件不存在 = 新会话，全零起步。
pub fn scan(log: &Path) -> Scan {
    let mut requests = 0u64;
    let mut max_turn = 0u64;
    let mut open_turn: Option<u64> = None;
    let mut open_ids: Vec<String> = Vec::new();
    let mut dangling: Vec<String> = Vec::new();
    let Ok(file) = std::fs::File::open(log) else {
        return Scan { next_turn: 1, open_turn: None, dangling };
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(frame) = serde_json::from_str::<Frame>(line.trim()) else { continue };
        match frame {
            Frame::Request(r) => {
                // 上一轮没收口就开了新轮：旧轮悬空调用进残骸
                if open_turn.is_some() {
                    dangling.append(&mut open_ids);
                }
                requests += 1;
                max_turn = max_turn.max(r.turn);
                open_turn = Some(r.turn);
            }
            Frame::ToolCall(c) => open_ids.push(c.id),
            Frame::ToolResult(r) => {
                // 先配本轮的回账；配不上 = 迟到的补账（追加在 done 之后
                // 的形状），回勾一笔悬空记录——这才让 repair 幂等
                if let Some(pos) = open_ids.iter().position(|id| *id == r.id) {
                    open_ids.remove(pos);
                } else if let Some(pos) = dangling.iter().position(|id| *id == r.id) {
                    dangling.remove(pos);
                }
            }
            Frame::Done(_) => {
                // done 关轮：没收齐的结果是已知残骸（崩溃收据形状）
                dangling.append(&mut open_ids);
                open_turn = None;
            }
            Frame::Steer(_) | Frame::Artifact(_) => {}
        }
    }
    // 尾部未收口轮的悬空调用同样算残骸
    dangling.append(&mut open_ids);
    Scan { next_turn: requests.max(max_turn) + 1, open_turn, dangling }
}

/// 补账：悬空调用补合成 tool_result、未收口轮补 done(interrupted)，
/// 全部从尾追加。返回补帧数（0 = 账已合法）。追加顺序先结果后收口，
/// 与重放折叠的配对规则一致。
pub fn repair(log: &Path) -> io::Result<usize> {
    let s = scan(log);
    let mut n = 0;
    for id in &s.dangling {
        append(
            log,
            &Frame::ToolResult(agi::ToolResult {
                id: id.clone(),
                ok: false,
                code: 1,
                out: String::new(),
                err: "(会话中断，server 补账：工具未回结果)".into(),
            }),
        )?;
        n += 1;
    }
    if let Some(turn) = s.open_turn {
        append(
            log,
            &Frame::Done(
                agi::Done::err("interrupted", "server 补账：上一轮未收口").at_turn(turn),
            ),
        )?;
        n += 1;
    }
    Ok(n)
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
            &Frame::Request(Request {
                avatar: "小满".into(),
                session: "main".into(),
                text: "你好".into(),
                turn: 1,
            }),
        )
        .unwrap();
        append(&log, &Frame::Done(Done::ok("好").at_turn(1))).unwrap();
        let lines: Vec<String> =
            std::fs::read_to_string(&log).unwrap().lines().map(String::from).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""t":"request""#));
        assert!(lines[1].contains(r#""t":"done""#));
    }

    // ---- scan / repair ----

    fn frames_to_lines(log: &Path) -> Vec<Frame> {
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l.trim()).unwrap())
            .collect()
    }

    #[test]
    fn scan_missing_file_is_fresh_start() {
        let d = tmp("scan-missing");
        let s = scan(&session_log(&d, "a", "main"));
        assert_eq!(s.next_turn, 1);
        assert_eq!(s.open_turn, None);
        assert!(s.dangling.is_empty());
    }

    #[test]
    fn repair_is_noop_on_legal_ledger() {
        let d = tmp("repair-clean");
        let log = session_log(&d, "a", "main");
        append(&log, &Frame::Request(Request {
            avatar: "a".into(), session: "main".into(), text: "问".into(), turn: 1,
        }))
        .unwrap();
        append(&log, &Frame::Done(Done::ok("答").at_turn(1))).unwrap();
        assert_eq!(repair(&log).unwrap(), 0);
        assert_eq!(frames_to_lines(&log).len(), 2);
    }

    #[test]
    fn repair_closes_open_turn_with_synthetic_done() {
        // server 死在半路：request 落账、无 done。下次 spawn 前 repair
        // 补 synthetic done(err, interrupted) 带回合号。
        let d = tmp("repair-open");
        let log = session_log(&d, "a", "main");
        append(&log, &Frame::Request(Request {
            avatar: "a".into(), session: "main".into(), text: "问1".into(), turn: 3,
        }))
        .unwrap();
        append(&log, &Frame::Done(Done::ok("答1").at_turn(3))).unwrap();
        append(&log, &Frame::Request(Request {
            avatar: "a".into(), session: "main".into(), text: "问2".into(), turn: 4,
        }))
        .unwrap();

        let before = scan(&log);
        assert_eq!(before.next_turn, 5);
        assert_eq!(before.open_turn, Some(4));

        assert_eq!(repair(&log).unwrap(), 1);
        let frames = frames_to_lines(&log);
        assert_eq!(frames.len(), 4);
        match &frames[3] {
            Frame::Done(d) => {
                assert!(!d.ok);
                assert_eq!(d.error.as_ref().unwrap().code, "interrupted");
                assert_eq!(d.turn, Some(4));
            }
            other => panic!("expected synthetic done, got {other:?}"),
        }
        // 补完再扫：账已合法，幂等
        let after = scan(&log);
        assert_eq!(after.open_turn, None);
        assert!(after.dangling.is_empty());
        assert_eq!(after.next_turn, 5);
        assert_eq!(repair(&log).unwrap(), 0);
    }

    #[test]
    fn repair_fills_dangling_tool_calls() {
        // 崩溃收据形状：done 已补但 tool_call 没等到结果（旧 close_broken
        // 只补 done）。repair 给悬空调用各补一帧合成 tool_result。
        let d = tmp("repair-dangling");
        let log = session_log(&d, "a", "main");
        append(&log, &Frame::Request(Request {
            avatar: "a".into(), session: "main".into(), text: "查".into(), turn: 1,
        }))
        .unwrap();
        append(&log, &Frame::ToolCall(agi::ToolCall {
            id: "c1".into(), tool: "web-search".into(), args: serde_json::json!(["x"]),
        }))
        .unwrap();
        append(&log, &Frame::ToolCall(agi::ToolCall {
            id: "c2".into(), tool: "ag-mem".into(), args: serde_json::json!(["kv"]),
        }))
        .unwrap();
        append(&log, &Frame::Done(Done::err("runtime_died", "eof").at_turn(1))).unwrap();

        let before = scan(&log);
        assert_eq!(before.open_turn, None); // done 在，轮已关
        assert_eq!(before.dangling, vec!["c1", "c2"]);

        assert_eq!(repair(&log).unwrap(), 2);
        let frames = frames_to_lines(&log);
        assert_eq!(frames.len(), 6);
        match &frames[4] {
            Frame::ToolResult(r) => {
                assert_eq!(r.id, "c1");
                assert!(!r.ok);
            }
            other => panic!("expected synthetic tool_result, got {other:?}"),
        }
        match &frames[5] {
            Frame::ToolResult(r) => assert_eq!(r.id, "c2"),
            other => panic!("expected synthetic tool_result, got {other:?}"),
        }
        assert_eq!(scan(&log).dangling, Vec::<String>::new());
    }

    #[test]
    fn repair_open_turn_with_pending_calls_results_then_done() {
        // 最深残骸：request + tool_call 后 server 直接死（无 done）。
        // 补账序 = 先补结果再补收口，重放折叠正好配对。
        let d = tmp("repair-deep");
        let log = session_log(&d, "a", "main");
        append(&log, &Frame::Request(Request {
            avatar: "a".into(), session: "main".into(), text: "查".into(), turn: 1,
        }))
        .unwrap();
        append(&log, &Frame::ToolCall(agi::ToolCall {
            id: "c1".into(), tool: "web-search".into(), args: serde_json::json!(["x"]),
        }))
        .unwrap();

        assert_eq!(repair(&log).unwrap(), 2);
        let frames = frames_to_lines(&log);
        assert_eq!(frames.len(), 4);
        assert!(matches!(&frames[2], Frame::ToolResult(r) if r.id == "c1"));
        assert!(matches!(&frames[3], Frame::Done(d) if d.turn == Some(1)));
        assert_eq!(repair(&log).unwrap(), 0);
    }

    #[test]
    fn legacy_ledger_continues_numbering_from_count() {
        // 旧账（无 turn 字段）：三帧 request 都读 turn=0，next_turn 按帧
        // 计数续成 4——不重写历史，号从今往后单调。
        let d = tmp("legacy-numbering");
        let log = session_log(&d, "a", "main");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let legacy = concat!(
            r#"{"t":"request","avatar":"a","session":"main","text":"q1"}"#, "\n",
            r#"{"t":"done","ok":true,"text":"a1","error":null}"#, "\n",
            r#"{"t":"request","avatar":"a","session":"main","text":"q2"}"#, "\n",
            r#"{"t":"done","ok":true,"text":"a2","error":null}"#, "\n",
            r#"{"t":"request","avatar":"a","session":"main","text":"q3"}"#, "\n",
            r#"{"t":"done","ok":true,"text":"a3","error":null}"#, "\n",
        );
        std::fs::write(&log, legacy).unwrap();
        let s = scan(&log);
        assert_eq!(s.next_turn, 4);
        assert_eq!(s.open_turn, None);
        assert_eq!(repair(&log).unwrap(), 0);
    }
}
