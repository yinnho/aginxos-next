//! 投递 ID 去重 LRU——外部系统（GitHub 等）在未及时 2xx 时会重试，同一
//! delivery 重放必须挡住，否则一轮 agent 白跑两次。形状照搬 ilink 的
//! `INBOUND_DEDUP`（TTL + 容量上限 + 过期清扫），压缩到本通道所需。

use std::time::{Duration, Instant};

use dashmap::DashMap;

pub struct DedupLru {
    map: DashMap<String, Instant>,
    ttl: Duration,
    max: usize,
}

impl DedupLru {
    pub fn new(ttl: Duration, max: usize) -> Self {
        Self {
            map: DashMap::new(),
            ttl,
            max,
        }
    }

    /// 首次见到该 key 返回 true（放行并记录）；TTL 内重复返回 false。
    pub fn check_and_mark(&self, key: &str) -> bool {
        let now = Instant::now();
        // 容量到顶先扫掉过期项；仍满则保守放行（去重是优化不是闸门）。
        if self.map.len() >= self.max {
            self.map.retain(|_, t| now.duration_since(*t) < self.ttl);
            if self.map.len() >= self.max {
                return true;
            }
        }
        !matches!(self.map.insert(key.to_string(), now), Some(prev) if now.duration_since(prev) < self.ttl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_seen_passes_repeat_blocks() {
        let lru = DedupLru::new(Duration::from_secs(60), 1000);
        assert!(lru.check_and_mark("a"));
        assert!(!lru.check_and_mark("a"));
        assert!(lru.check_and_mark("b"));
    }

    #[tokio::test]
    async fn expired_key_passes_again() {
        let lru = DedupLru::new(Duration::from_millis(30), 1000);
        assert!(lru.check_and_mark("a"));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(lru.check_and_mark("a"));
    }

    #[test]
    fn full_map_degrades_open() {
        let lru = DedupLru::new(Duration::from_secs(60), 2);
        assert!(lru.check_and_mark("a"));
        assert!(lru.check_and_mark("b"));
        // 满+全新鲜：保守放行，不误杀。
        assert!(lru.check_and_mark("c"));
    }
}
