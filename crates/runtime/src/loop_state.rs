// loop_state — 轮内状态（carrier agent_loop/state.rs + helpers.rs 的
// 检测器搬入）。这里没有 I/O，全是纯计数与判定。
//
// 搬入的纪律（每一条都是老仓实收据换来的）：
// - 无进展检测：连续 3 轮既无成功工具也无终答也无 MaxTokens 生成 →
//   判卡死终止（成功工具才算进展：整轮工具全失败 = 空转）。
// - 硬循环：同 (工具名, 输入哈希) 连续 4 次 → 终止。同名不同参（分页）
//   不算硬循环。
// - 软循环：同工具名连续 2 次 → 状态消息里轻提醒（不摘工具）。
// - 累积循环：同 (名, 哈希) 整轮累计 3 次提醒、5 次加重语气、8 次终止
//   ——抓「轮换式重复」（4 个路径轮着读，每个都没连满 4 次）。
//   被拒/失败的工具调用也计数：锤一个不存在的命令正是该断的循环。
// - 上下文压力：用量占比 ≥50%/70%/85% → elevated/high/critical，高压时
//   状态消息提醒收束。

use std::collections::HashMap;

/// 连续无进展轮的上限（无工具调用、无最终答案、非 MaxTokens 生成中）。
pub const NO_PROGRESS_THRESHOLD: u32 = 3;

/// 硬循环窗口：同 (name, input_hash) 连续这么多次 → 终止。
pub const LOOP_DETECTION_WINDOW: usize = 4;

/// 软循环窗口：同工具名连续这么多次 → 状态消息轻提醒。
pub const SOFT_LOOP_WINDOW: usize = 2;

/// 累积式循环阈值（整轮计，抓轮换式重复）。
pub const CUMULATIVE_REMIND_AT: u32 = 3;
pub const CUMULATIVE_ESCALATE_AT: u32 = 5;
pub const CUMULATIVE_BREAK_AT: u32 = 8;

/// 历史消息条数上限（超了从最前头成对裁）。
pub const MAX_HISTORY_MESSAGES: usize = 30;

/// 默认上下文窗口（token）。
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

/// 单条工具结果在喂给 brain 前的截断上限（字符）。CLI 工具可能倾倒
/// 巨量输出，不截就是上下文炸弹。
pub const TOOL_RESULT_MAX_CHARS: usize = 12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextPressure {
    #[default]
    Normal,
    Elevated,
    High,
    Critical,
}

impl ContextPressure {
    pub fn as_label(&self) -> &'static str {
        match self {
            ContextPressure::Normal => "normal",
            ContextPressure::Elevated => "elevated",
            ContextPressure::High => "high",
            ContextPressure::Critical => "critical",
        }
    }

    pub fn from_usage_pct(pct: f64) -> ContextPressure {
        if pct >= 0.85 {
            ContextPressure::Critical
        } else if pct >= 0.70 {
            ContextPressure::High
        } else if pct >= 0.50 {
            ContextPressure::Elevated
        } else {
            ContextPressure::Normal
        }
    }
}

/// 每工具滑动窗（5 次）的成败账：连续失败的工具在状态消息里点名。
#[derive(Debug, Clone, Default)]
pub struct ToolErrorTracker {
    history: HashMap<String, Vec<bool>>,
}

impl ToolErrorTracker {
    pub fn record(&mut self, tool_name: &str, success: bool) {
        let entry = self.history.entry(tool_name.to_string()).or_default();
        entry.push(success);
        if entry.len() > 5 {
            entry.remove(0);
        }
    }

    pub fn consecutive_failures(&self, tool_name: &str) -> u32 {
        let Some(h) = self.history.get(tool_name) else { return 0 };
        h.iter().rev().take_while(|s| !**s).count() as u32
    }

    pub fn failed_tools(&self) -> Vec<(String, u32)> {
        self.history
            .keys()
            .filter_map(|name| {
                let cf = self.consecutive_failures(name);
                if cf > 0 {
                    Some((name.clone(), cf))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoopState {
    pub iteration: u32,
    pub idle_streak: u32,
    /// 本轮迭代内成功执行的工具数（整轮全失败 = 空转）。
    pub tools_this_iter: u32,
    pub any_tools_executed: bool,
    pub context_tokens_used_estimate: usize,
    pub context_tokens_max: usize,
    pub context_pressure: ContextPressure,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub recent_tool_calls: Vec<(String, u64)>,
    pub tool_call_counts: HashMap<(String, u64), u32>,
    pub error_tracker: ToolErrorTracker,
    pub consecutive_max_tokens: u32,
    pub text_recovery_retries: u32,
    pub text_recovery_final: bool,
}

impl LoopState {
    pub fn new(context_window_tokens: usize) -> LoopState {
        LoopState {
            context_tokens_max: context_window_tokens,
            ..LoopState::default()
        }
    }

    pub fn context_usage_pct(&self) -> f64 {
        if self.context_tokens_max == 0 {
            0.0
        } else {
            self.context_tokens_used_estimate as f64 / self.context_tokens_max as f64
        }
    }

    /// 记一轮是否进展；到阈值返回 Some(idle_streak)，调用方据此终止。
    pub fn record_iteration_progress(&mut self, made_progress: bool) -> Option<u32> {
        if made_progress {
            self.idle_streak = 0;
            return None;
        }
        self.idle_streak += 1;
        if self.idle_streak >= NO_PROGRESS_THRESHOLD {
            Some(self.idle_streak)
        } else {
            None
        }
    }

    /// 每轮注入给模型的运行状态（中文，形状照搬 carrier——实战调出来的
    /// 提示语气不动）。
    pub fn build_status_message(&self) -> String {
        let mut msg = format!(
            "📊 Turn {} | 📐 context: {} ({}%)",
            self.iteration + 1,
            self.context_pressure.as_label(),
            (self.context_usage_pct() * 100.0) as u32,
        );
        if let Some(name) = detect_soft_loop(&self.recent_tool_calls, SOFT_LOOP_WINDOW) {
            msg.push_str(&format!(
                "\n💡 工具 `{name}` 连续被调用，确认这不是重复操作？如果是分页/批量则忽略。"
            ));
        }
        let failed: Vec<String> = self
            .error_tracker
            .failed_tools()
            .iter()
            .map(|(name, count)| format!("{name}(×{count})"))
            .collect();
        if !failed.is_empty() {
            msg.push_str(&format!("\n⚠️ 连续出错: {}", failed.join(", ")));
        }
        if matches!(self.context_pressure, ContextPressure::High | ContextPressure::Critical) {
            msg.push_str("\n⚠️ 上下文即将耗尽，优先输出最终答案，减少工具调用。");
        }
        msg
    }
}

// ---------------------------------------------------------------------------
// 循环检测器（纯函数，搬 carrier helpers）
// ---------------------------------------------------------------------------

/// 工具输入的哈希。serde_json 对象键按字典序序列化，`{"a":1,"b":2}` 与
/// `{"b":2,"a":1}` 同哈希；数组顺序保留（有语义）。
pub fn tool_input_hash(input: &serde_json::Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let serialized = serde_json::to_string(input).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    hasher.finish()
}

/// 尾部 window 条全是同一 (name, hash) → 硬循环，返回之。
pub fn detect_tool_loop(recent: &[(String, u64)], window: usize) -> Option<(String, u64)> {
    if recent.len() < window {
        return None;
    }
    let tail = &recent[recent.len() - window..];
    let first = &tail[0];
    if tail.iter().all(|e| e == first) {
        Some(first.clone())
    } else {
        None
    }
}

/// 尾部 window 条同工具名（不论参数）→ 软循环，返回工具名。
pub fn detect_soft_loop(recent: &[(String, u64)], window: usize) -> Option<String> {
    if recent.len() < window {
        return None;
    }
    let tail = &recent[recent.len() - window..];
    let first_name = &tail[0].0;
    if tail.iter().all(|(name, _)| name == first_name) {
        Some(first_name.clone())
    } else {
        None
    }
}

/// 给循环警告用的参数预览（截断）。
pub fn tool_args_preview(input: &serde_json::Value, max_chars: usize) -> String {
    let s = serde_json::to_string(input).unwrap_or_default();
    if s.chars().count() <= max_chars {
        s
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_thresholds() {
        assert_eq!(ContextPressure::from_usage_pct(0.10), ContextPressure::Normal);
        assert_eq!(ContextPressure::from_usage_pct(0.50), ContextPressure::Elevated);
        assert_eq!(ContextPressure::from_usage_pct(0.70), ContextPressure::High);
        assert_eq!(ContextPressure::from_usage_pct(0.85), ContextPressure::Critical);
    }

    #[test]
    fn no_progress_threshold_trips_on_third_idle() {
        let mut s = LoopState::new(1000);
        assert_eq!(s.record_iteration_progress(false), None);
        assert_eq!(s.record_iteration_progress(false), None);
        assert_eq!(s.record_iteration_progress(false), Some(3));
        assert_eq!(s.record_iteration_progress(true), None);
        assert_eq!(s.record_iteration_progress(false), None);
    }

    #[test]
    fn hard_loop_detected_but_pagination_not() {
        let same = vec![("t".to_string(), 7u64); 4];
        assert!(detect_tool_loop(&same, LOOP_DETECTION_WINDOW).is_some());
        // 同名不同参（分页）：不是硬循环
        let paged = vec![
            ("t".to_string(), 1u64),
            ("t".to_string(), 2u64),
            ("t".to_string(), 3u64),
            ("t".to_string(), 4u64),
        ];
        assert!(detect_tool_loop(&paged, LOOP_DETECTION_WINDOW).is_none());
        // 但同名连续 → 软循环点名
        assert_eq!(detect_soft_loop(&paged, SOFT_LOOP_WINDOW).as_deref(), Some("t"));
    }

    #[test]
    fn input_hash_key_order_insensitive() {
        let a = serde_json::from_str::<serde_json::Value>(r#"{"a":1,"b":2}"#).unwrap();
        let b = serde_json::from_str::<serde_json::Value>(r#"{"b":2,"a":1}"#).unwrap();
        assert_eq!(tool_input_hash(&a), tool_input_hash(&b));
        let c = serde_json::json!([1, 2]);
        let d = serde_json::json!([2, 1]);
        assert_ne!(tool_input_hash(&c), tool_input_hash(&d)); // 数组顺序有语义
    }

    #[test]
    fn error_tracker_sliding_window() {
        let mut t = ToolErrorTracker::default();
        for _ in 0..3 {
            t.record("a", false);
        }
        assert_eq!(t.consecutive_failures("a"), 3);
        t.record("a", true);
        assert_eq!(t.consecutive_failures("a"), 0);
        assert!(t.failed_tools().is_empty());
    }

    #[test]
    fn status_message_lists_failed_tools() {
        let mut s = LoopState::new(1000);
        s.error_tracker.record("web-search", false);
        s.error_tracker.record("web-search", false);
        let msg = s.build_status_message();
        assert!(msg.contains("📊 Turn 1"));
        assert!(msg.contains("web-search(×2)"));
    }
}
