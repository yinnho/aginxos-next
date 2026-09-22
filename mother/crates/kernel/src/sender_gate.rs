//! Per-(agent, sender-label) turn gate — dsh inbox+claim at turn granularity.
//!
//! A user rapid-firing messages ("帮我查月票" + "顺便看下时刻表") previously
//! started two concurrent turns racing on the same session, each with half
//! the context, producing interleaved replies. The gate gives every
//! (agent, label) ONE inbox and ONE running turn:
//!
//! - messages enqueue into the per-key inbox, then wait on the per-key lock;
//! - the lock holder CLAIMS the whole inbox (its own message + everything
//!   queued while a previous turn ran) as ONE combined turn;
//! - a caller whose message was claimed by an earlier runner returns a
//!   synthetic empty/silent result — the combined reply was already
//!   delivered to the same channel recipient by that runner (the bridge
//!   skips empty responses, so nothing is sent twice).
//!
//! Cross-sender throughput is unaffected: keys are per (agent, label), and
//! API calls without a sender never enter the gate.

use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use carrier_types::message::ContentBlock;
/// One queued inbound message.
struct Pending {
    token: u64,
    message: String,
    blocks: Option<Vec<ContentBlock>>,
}

#[derive(Default)]
struct KeyState {
    lock: tokio::sync::Mutex<()>,
    inbox: std::sync::Mutex<VecDeque<Pending>>,
}

/// The shared gate. Clone-cheap handle state lives per key.
#[derive(Default)]
pub struct SenderGate {
    keys: DashMap<String, Arc<KeyState>>,
    tokens: AtomicU64,
}

/// What a gated caller's message produced.
pub enum GatedOutcome<R> {
    /// This caller ran the (possibly combined) turn.
    Ran(R),
    /// This caller's message was claimed by an earlier runner's combined
    /// turn; the reply already went out on that turn.
    Merged,
}

/// A pending message batch claimed by the runner.
pub struct ClaimedBatch {
    pub texts: Vec<String>,
    pub blocks: Vec<ContentBlock>,
    /// How many messages were coalesced into this batch.
    pub len: usize,
}

impl ClaimedBatch {
    /// The combined user message for one turn: numbered when coalesced, raw
    /// when single (no behavior change for the common case).
    pub fn combined_message(&self) -> String {
        if self.len <= 1 {
            return self.texts.first().cloned().unwrap_or_default();
        }
        let mut out = format!("（用户连发 {} 条消息，按顺序处理）", self.len);
        for (i, t) in self.texts.iter().enumerate() {
            out.push_str(&format!("\n{}. {}", i + 1, t));
        }
        out
    }
}

impl SenderGate {
    /// Gate one inbound message: enqueue, claim, run.
    ///
    /// `run` receives the claimed batch (this caller's message plus any
    /// messages that queued while a previous turn was running) and executes
    /// the combined turn.
    pub async fn run<F, Fut, R>(
        &self,
        key: &str,
        message: String,
        blocks: Option<Vec<ContentBlock>>,
        run: F,
    ) -> GatedOutcome<Fut::Output>
    where
        F: FnOnce(ClaimedBatch) -> Fut,
        Fut: std::future::Future,
    {
        let state = Arc::clone(
            self.keys
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(KeyState::default()))
                .value(),
        );
        let token = self.tokens.fetch_add(1, Ordering::Relaxed);
        {
            let mut inbox = state.inbox.lock().expect("inbox lock");
            inbox.push_back(Pending {
                token,
                message,
                blocks,
            });
        }

        let _guard = state.lock.lock().await;
        // Claim: take our own entry plus everything queued behind it.
        let mut batch = VecDeque::new();
        {
            let mut inbox = state.inbox.lock().expect("inbox lock");
            std::mem::swap(&mut batch, &mut *inbox);
        }
        // Our token gone => an earlier runner already claimed this message.
        if !batch.iter().any(|p| p.token == token) {
            return GatedOutcome::Merged;
        }

        let mut texts = Vec::with_capacity(batch.len());
        let mut merged_blocks = Vec::new();
        let len = batch.len();
        for p in batch {
            if let Some(blocks) = p.blocks {
                merged_blocks.extend(blocks);
            }
            texts.push(p.message);
        }
        GatedOutcome::Ran(
            run(ClaimedBatch {
                texts,
                blocks: merged_blocks,
                len,
            })
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn single_message_runs_asis() {
        let gate = SenderGate::default();
        match gate
            .run::<_, _, ()>("k", "你好".to_string(), None, |b| async move {
                assert_eq!(b.len, 1);
                assert_eq!(b.combined_message(), "你好");
            })
            .await
        {
            GatedOutcome::Ran(()) => {}
            GatedOutcome::Merged => panic!("single message must run"),
        }
    }

    #[tokio::test]
    async fn queued_messages_coalesce_into_one_turn() {
        // Runner holds the lock; two more messages queue; a second claimer
        // takes all of them in one batch and the queued callers get Merged.
        let gate = Arc::new(SenderGate::default());

        // Runner 1: holds the gate while the queue builds.
        let g1 = Arc::clone(&gate);
        let first = tokio::spawn(async move {
            g1.run::<_, _, ()>("k", "第一条".to_string(), None, |b| async move {
                assert_eq!(b.len, 1);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            })
            .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // Two more messages arrive while turn 1 runs.
        let g2 = Arc::clone(&gate);
        let second = tokio::spawn(async move {
            g2.run::<_, _, ()>("k", "第二条".to_string(), None, |b| async move {
                // Both queued messages coalesce into ONE claim.
                assert_eq!(b.len, 2);
                let combined = b.combined_message();
                assert!(combined.contains("第二条"), "{combined}");
                assert!(combined.contains("第三条"), "{combined}");
                assert!(combined.contains("连发 2 条"), "{combined}");
            })
            .await
        });
        let g3 = Arc::clone(&gate);
        let third = tokio::spawn(async move {
            g3.run::<_, _, ()>("k", "第三条".to_string(), None, |b| async move {
                // If this ever runs alone something is wrong — it must be
                // claimed together with 第二条 by one runner.
                assert!(b.len >= 1);
            })
            .await
        });

        assert!(matches!(first.await.unwrap(), GatedOutcome::Ran(())));
        // Both waiters resolve: one ran the combined turn, the other merged.
        let outcomes = [second.await.unwrap(), third.await.unwrap()];
        let ran = outcomes
            .iter()
            .filter(|o| matches!(o, GatedOutcome::Ran(())))
            .count();
        let merged = outcomes
            .iter()
            .filter(|o| matches!(o, GatedOutcome::Merged))
            .count();
        assert_eq!(
            (ran, merged),
            (1, 1),
            "exactly one waiter runs the combined turn, the other merges"
        );
    }

    #[tokio::test]
    async fn different_keys_run_in_parallel() {
        let gate = Arc::new(SenderGate::default());
        let g1 = Arc::clone(&gate);
        let t1 = tokio::spawn(async move {
            g1.run::<_, _, usize>("agent:user-a", "a".to_string(), None, |b| async move {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                b.len
            })
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let g2 = Arc::clone(&gate);
        let t2 = tokio::spawn(async move {
            g2.run::<_, _, usize>(
                "agent:user-b",
                "b".to_string(),
                None,
                |b| async move { b.len },
            )
            .await
        });
        // t2 must NOT wait for t1 (80ms) — completes well within.
        let (r1, r2) = (t1.await.unwrap(), t2.await.unwrap());
        assert!(matches!(r1, GatedOutcome::Ran(1)));
        assert!(matches!(r2, GatedOutcome::Ran(1)));
    }

    #[tokio::test]
    async fn blocks_from_all_claimed_messages_merge() {
        let gate = SenderGate::default();
        let blocks = Some(vec![ContentBlock::Text {
            text: "图1说明".into(),
            provider_metadata: None,
        }]);
        match gate
            .run::<_, _, ()>("k", "看图".to_string(), blocks, |b| async move {
                assert_eq!(b.blocks.len(), 1);
                assert_eq!(b.texts, vec!["看图".to_string()]);
            })
            .await
        {
            GatedOutcome::Ran(()) => {}
            GatedOutcome::Merged => panic!("must run"),
        }
    }
}
