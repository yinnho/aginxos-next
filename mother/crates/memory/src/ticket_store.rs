//! 用户侧票据仓库（借用机制待建清单 #5）。
//!
//! 借用机制的会话真源在用户侧：App/桌面/CLI 把借道轮返回的
//! [`SessionTicket`] 存在这里，下次借用按 `(agent_name, label)` 取回提交。
//! 服务器对借用者零持久化，本仓库就是"用户的 session 数据库"。
//!
//! 与 [`crate::session::SessionStore`]（主人服务器的会话表）语义相反：
//! 那边存服务器自己的对话，这边存用户借别人分身攒下的票据。
//!
//! Mirrors `ChainResumeStore`：`Arc<Mutex<Connection>>` + sync rusqlite。

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::{CarrierError, CarrierResult};

use crate::session::SessionTicket;

/// 一条票据的元信息（列表用，不含消息体）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TicketEntry {
    pub agent_name: String,
    pub label: String,
    pub message_count: i64,
    pub updated_at: String,
}

/// SQLite-backed user-side ticket store.
#[derive(Clone)]
pub struct TicketStore {
    conn: Arc<Mutex<Connection>>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl TicketStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// 保存（upsert）一张票据。`label` 为空时落 "default"。
    pub fn save(
        &self,
        agent_name: &str,
        label: &str,
        ticket: &SessionTicket,
    ) -> CarrierResult<()> {
        let label = if label.trim().is_empty() { "default" } else { label };
        let json = serde_json::to_string(ticket)
            .map_err(|e| CarrierError::Internal(format!("ticket serialize: {e}")))?;
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO borrow_tickets (agent_name, label, ticket_json, message_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(agent_name, label) DO UPDATE SET
                ticket_json=excluded.ticket_json,
                message_count=excluded.message_count,
                updated_at=excluded.updated_at",
            rusqlite::params![
                agent_name,
                label,
                json,
                ticket.messages.len() as i64,
                now_rfc3339()
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// 取回一张票据；不存在返回 None。
    pub fn load(&self, agent_name: &str, label: &str) -> CarrierResult<Option<SessionTicket>> {
        let label = if label.trim().is_empty() { "default" } else { label };
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let json: Option<String> = conn
            .query_row(
                "SELECT ticket_json FROM borrow_tickets WHERE agent_name=?1 AND label=?2",
                rusqlite::params![agent_name, label],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(CarrierError::Memory(other.to_string())),
            })?;
        let Some(json) = json else {
            return Ok(None);
        };
        serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| CarrierError::Internal(format!("ticket deserialize: {e}")))
    }

    /// 删除一张票据（用户侧"开新对话"）。
    pub fn delete(&self, agent_name: &str, label: &str) -> CarrierResult<bool> {
        let label = if label.trim().is_empty() { "default" } else { label };
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let n = conn
            .execute(
                "DELETE FROM borrow_tickets WHERE agent_name=?1 AND label=?2",
                rusqlite::params![agent_name, label],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(n > 0)
    }

    /// 某分身下的全部票据元信息（按更新时间倒序）。
    pub fn list(&self, agent_name: &str) -> CarrierResult<Vec<TicketEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT label, message_count, updated_at FROM borrow_tickets
                 WHERE agent_name=?1 ORDER BY updated_at DESC",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_name], |row| {
                Ok(TicketEntry {
                    agent_name: agent_name.to_string(),
                    label: row.get(0)?,
                    message_count: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| CarrierError::Memory(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionTicket;

    fn store() -> TicketStore {
        let conn = Connection::open_in_memory().unwrap();
        crate::migration::run_migrations(&conn).unwrap();
        TicketStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn save_load_roundtrip_and_upsert() {
        let s = store();
        let mut t = SessionTicket::empty(Some("borrow:gaokao-advisor".into()));
        t.messages.push(carrier_types::message::Message::user("hi"));
        t.messages.push(carrier_types::message::Message::assistant("hello"));

        s.save("gaokao-advisor", "", &t).unwrap();
        let loaded = s.load("gaokao-advisor", "default").unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.label.as_deref(), Some("borrow:gaokao-advisor"));

        // upsert：第二轮长出的消息覆盖旧票据。
        t.messages.push(carrier_types::message::Message::user("again"));
        s.save("gaokao-advisor", "default", &t).unwrap();
        let loaded = s.load("gaokao-advisor", "default").unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 3);
    }

    #[test]
    fn load_missing_is_none() {
        let s = store();
        assert!(s.load("nope", "default").unwrap().is_none());
    }

    #[test]
    fn list_and_delete() {
        let s = store();
        let t = SessionTicket::empty(None);
        s.save("a", "work", &t).unwrap();
        s.save("a", "play", &t).unwrap();
        s.save("b", "x", &t).unwrap();

        let entries = s.list("a").unwrap();
        assert_eq!(entries.len(), 2);
        let mut labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        labels.sort();
        assert_eq!(labels, vec!["play", "work"]);
        assert_eq!(entries[0].message_count, 0);

        assert!(s.delete("a", "work").unwrap());
        assert!(!s.delete("a", "work").unwrap());
        assert_eq!(s.list("a").unwrap().len(), 1);
    }
}
