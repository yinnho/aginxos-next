//! Persistent automation-rule store backed by SQLite.
//!
//! Mirrors `CronJobStore` (`cron_store.rs`): `Arc<Mutex<Connection>>` + sync
//! rusqlite bodies. The substrate wraps these in `spawn_blocking` async fns.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::automation::{AutomationRule, TaskKind, TriggerKind};
use carrier_types::error::{CarrierError, CarrierResult};

/// SQLite-backed automation-rule store.
#[derive(Clone)]
pub struct AutomationRuleStore {
    conn: Arc<Mutex<Connection>>,
}

impl AutomationRuleStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// All rules for `(channel, app_id)` ordered highest-priority-first then
    /// oldest. Includes disabled rules (callers filter `enabled` as needed) -
    /// the admin `list` tool wants to see disabled rules too.
    pub fn list_by_app(&self, channel: &str, app_id: &str) -> CarrierResult<Vec<AutomationRule>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, app_id, channel, name, enabled, priority, trigger_kind, trigger_data, \
                        task_kind, task_payload, created_at, updated_at, target \
                 FROM automation_rules \
                 WHERE channel=?1 AND app_id=?2 \
                 ORDER BY priority DESC, created_at ASC",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![channel, app_id], |row| {
                Ok(RowData {
                    id: row.get(0)?,
                    app_id: row.get(1)?,
                    channel: row.get(2)?,
                    name: row.get(3)?,
                    enabled: row.get(4)?,
                    priority: row.get(5)?,
                    trigger_kind: row.get(6)?,
                    trigger_data: row.get(7)?,
                    task_kind: row.get(8)?,
                    task_payload: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    target: row.get(12)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let mut rules = Vec::new();
        for row in rows {
            let r = row.map_err(|e| CarrierError::Memory(e.to_string()))?;
            rules.push(row_to_rule(r)?);
        }
        Ok(rules)
    }

    /// Insert or update by id. Does NOT overwrite `created_at` (only refreshes
    /// `updated_at`) so priority ordering ties stay stable across edits.
    pub fn upsert(&self, rule: &AutomationRule) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let trigger_kind = rule.trigger_kind.as_str();
        let task_kind = rule.task_kind.as_str();
        let task_payload = serde_json::to_string(&rule.task_payload)
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO automation_rules (id, app_id, channel, name, enabled, priority, \
                                            trigger_kind, trigger_data, task_kind, task_payload, \
                                            created_at, updated_at, target) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
             ON CONFLICT(id) DO UPDATE SET \
               app_id=?2, channel=?3, name=?4, enabled=?5, priority=?6, \
               trigger_kind=?7, trigger_data=?8, task_kind=?9, task_payload=?10, \
               updated_at=?12, target=?13",
            rusqlite::params![
                rule.id,
                rule.app_id,
                rule.channel,
                rule.name,
                rule.enabled as i32,
                rule.priority,
                trigger_kind,
                rule.trigger_data,
                task_kind,
                task_payload,
                rule.created_at,
                rule.updated_at,
                rule.target,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete a rule by id.
    pub fn delete(&self, id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM automation_rules WHERE id=?1",
            rusqlite::params![id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }
}

struct RowData {
    id: String,
    app_id: String,
    channel: String,
    name: String,
    enabled: i32,
    priority: i64,
    trigger_kind: String,
    trigger_data: String,
    task_kind: String,
    task_payload: String,
    created_at: String,
    updated_at: String,
    target: String,
}

fn row_to_rule(r: RowData) -> CarrierResult<AutomationRule> {
    let trigger_kind = match r.trigger_kind.as_str() {
        "subscribe" => TriggerKind::Subscribe,
        "keyword" => TriggerKind::Keyword,
        "menu_click" => TriggerKind::MenuClick,
        "scan" => TriggerKind::Scan,
        other => {
            return Err(CarrierError::Serialization(format!(
                "unknown trigger_kind {other}"
            )))
        }
    };
    let task_kind = match r.task_kind.as_str() {
        "push_text" => TaskKind::PushText,
        "push_miniprogram" => TaskKind::PushMiniprogram,
        "notify_admin" => TaskKind::NotifyAdmin,
        "push" => TaskKind::Push,
        other => {
            return Err(CarrierError::Serialization(format!(
                "unknown task_kind {other}"
            )))
        }
    };
    let task_payload: serde_json::Value = serde_json::from_str(&r.task_payload)
        .map_err(|e| CarrierError::Serialization(format!("bad task_payload: {e}")))?;
    Ok(AutomationRule {
        id: r.id,
        app_id: r.app_id,
        channel: r.channel,
        name: r.name,
        enabled: r.enabled != 0,
        priority: r.priority,
        trigger_kind,
        trigger_data: r.trigger_data,
        task_kind,
        task_payload,
        target: r.target,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn test_store() -> AutomationRuleStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        AutomationRuleStore::new(std::sync::Arc::new(std::sync::Mutex::new(conn)))
    }

    fn rule(
        id: &str,
        app: &str,
        trig: TriggerKind,
        task: TaskKind,
        prio: i64,
        enabled: bool,
    ) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            app_id: app.to_string(),
            channel: "weixin-oa".to_string(),
            name: format!("rule {id}"),
            enabled,
            priority: prio,
            trigger_kind: trig,
            trigger_data: match trig {
                TriggerKind::Keyword | TriggerKind::MenuClick | TriggerKind::Scan => {
                    "月卡".to_string()
                }
                TriggerKind::Subscribe => String::new(),
            },
            task_kind: task,
            task_payload: serde_json::json!({"text":"hi"}),
            created_at: "2026-08-04T00:00:00Z".to_string(),
            updated_at: "2026-08-04T00:00:00Z".to_string(),
            target: "current".to_string(),
        }
    }

    #[test]
    fn list_by_app_priority_order_includes_disabled() {
        // list_by_app returns ALL rules (enabled+disabled); the callback filters
        // enabled in Rust so the admin `list` tool can see disabled rules too.
        let store = test_store();
        store
            .upsert(&rule(
                "a",
                "app1",
                TriggerKind::Keyword,
                TaskKind::PushText,
                5,
                true,
            ))
            .unwrap();
        store
            .upsert(&rule(
                "b",
                "app1",
                TriggerKind::Subscribe,
                TaskKind::PushText,
                10,
                true,
            ))
            .unwrap();
        store
            .upsert(&rule(
                "c",
                "app1",
                TriggerKind::Keyword,
                TaskKind::PushText,
                1,
                false,
            ))
            .unwrap();
        store
            .upsert(&rule(
                "d",
                "app2",
                TriggerKind::Subscribe,
                TaskKind::PushText,
                100,
                true,
            ))
            .unwrap();

        let rules = store.list_by_app("weixin-oa", "app1").unwrap();
        assert_eq!(rules.len(), 3); // a, b, c (d is app2)
        assert_eq!(rules[0].id, "b"); // priority 10
        assert_eq!(rules[1].id, "a"); // priority 5
        assert_eq!(rules[2].id, "c"); // priority 1
    }

    #[test]
    fn upsert_updates_existing_preserving_created_at() {
        let store = test_store();
        let mut first = rule(
            "a",
            "app1",
            TriggerKind::Keyword,
            TaskKind::PushText,
            0,
            true,
        );
        first.created_at = "ORIGINAL".to_string();
        store.upsert(&first).unwrap();

        // Update: change fields, attempt to overwrite created_at (must be ignored).
        let mut updated = rule(
            "a",
            "app1",
            TriggerKind::Subscribe,
            TaskKind::PushMiniprogram,
            5,
            true,
        );
        updated.created_at = "SHOULD_BE_IGNORED".to_string();
        updated.updated_at = "NEW".to_string();
        store.upsert(&updated).unwrap();

        let rules = store.list_by_app("weixin-oa", "app1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].trigger_kind, TriggerKind::Subscribe);
        assert_eq!(rules[0].task_kind, TaskKind::PushMiniprogram);
        assert_eq!(rules[0].priority, 5);
        assert_eq!(rules[0].created_at, "ORIGINAL"); // ON CONFLICT preserves created_at
        assert_eq!(rules[0].updated_at, "NEW");
    }

    #[test]
    fn delete_removes_rule() {
        let store = test_store();
        store
            .upsert(&rule(
                "a",
                "app1",
                TriggerKind::Subscribe,
                TaskKind::PushText,
                0,
                true,
            ))
            .unwrap();
        store.delete("a").unwrap();
        assert!(store.list_by_app("weixin-oa", "app1").unwrap().is_empty());
    }
}
