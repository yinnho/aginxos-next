//! Score store for chunk admission scoring.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};

use super::types::ScoreSignals;

/// Result of a score lookup.
pub struct ScoreRow {
    pub signals: ScoreSignals,
    pub total: f32,
    pub dropped: bool,
    pub reason: Option<String>,
}

/// Score store backed by SQLite.
#[derive(Clone)]
pub struct ScoreStore {
    conn: Arc<Mutex<Connection>>,
}

impl ScoreStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Write a score row for a chunk.
    pub fn write_score(
        &self,
        owner_id: &str,
        chunk_id: &str,
        signals: &ScoreSignals,
        total: f32,
        dropped: bool,
        reason: Option<&str>,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT OR REPLACE INTO mem_tree_score
             (chunk_id, owner_id, total, token_count_signal, unique_words_signal,
              metadata_weight, source_weight, interaction_weight, entity_density,
              llm_importance, llm_importance_reason, dropped, reason, computed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                chunk_id,
                owner_id,
                total,
                signals.token_count,
                signals.unique_words,
                signals.metadata_weight,
                signals.source_weight,
                signals.interaction,
                signals.entity_density,
                signals.llm_importance,
                reason,
                dropped as i32,
                reason,
                now_ms,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Get the score for a chunk.
    pub fn get_score(&self, owner_id: &str, chunk_id: &str) -> CarrierResult<Option<ScoreRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let result = conn.query_row(
            "SELECT token_count_signal, unique_words_signal, metadata_weight,
                    source_weight, interaction_weight, entity_density, llm_importance,
                    total, dropped, reason
             FROM mem_tree_score WHERE owner_id = ?1 AND chunk_id = ?2",
            rusqlite::params![owner_id, chunk_id],
            |row| {
                let dropped: i32 = row.get(8)?;
                let reason: Option<String> = row.get(9)?;
                Ok(ScoreRow {
                    signals: ScoreSignals {
                        token_count: row.get(0)?,
                        unique_words: row.get(1)?,
                        metadata_weight: row.get(2)?,
                        source_weight: row.get(3)?,
                        interaction: row.get(4)?,
                        entity_density: row.get(5)?,
                        llm_importance: row.get(6)?,
                    },
                    total: row.get::<_, f32>(7)?,
                    dropped: dropped != 0,
                    reason,
                })
            },
        );

        match result {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Get the LLM importance for a chunk (used for re-scoring).
    pub fn get_llm_importance(
        &self,
        owner_id: &str,
        chunk_id: &str,
    ) -> CarrierResult<Option<(f32, Option<String>)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let result = conn.query_row(
            "SELECT llm_importance, llm_importance_reason FROM mem_tree_score WHERE owner_id = ?1 AND chunk_id = ?2",
            rusqlite::params![owner_id, chunk_id],
            |row| {
                let importance: f32 = row.get(0)?;
                let reason: Option<String> = row.get(1)?;
                Ok((importance, reason))
            },
        );

        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Update the LLM importance for a chunk.
    pub fn set_llm_importance(
        &self,
        owner_id: &str,
        chunk_id: &str,
        importance: f32,
        reason: Option<&str>,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute(
            "UPDATE mem_tree_score SET llm_importance = ?1, llm_importance_reason = ?2
             WHERE owner_id = ?3 AND chunk_id = ?4",
            rusqlite::params![importance, reason, owner_id, chunk_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> ScoreStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        ScoreStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_write_and_get_score() {
        let store = setup();
        let signals = ScoreSignals {
            token_count: 0.5,
            unique_words: 0.6,
            metadata_weight: 0.7,
            source_weight: 0.8,
            interaction: 0.9,
            entity_density: 0.4,
            llm_importance: 0.0,
        };

        store
            .write_score(
                "owner_1",
                "chunk_001",
                &signals,
                0.75,
                false,
                Some("high quality"),
            )
            .unwrap();

        let row = store.get_score("owner_1", "chunk_001").unwrap().unwrap();

        assert!((row.total - 0.75).abs() < 0.01);
        assert!(!row.dropped);
        assert_eq!(row.reason, Some("high quality".to_string()));
        assert!((row.signals.token_count - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_get_missing_score() {
        let store = setup();
        assert!(store.get_score("owner_1", "nope").unwrap().is_none());
    }

    #[test]
    fn test_set_llm_importance() {
        let store = setup();
        let signals = ScoreSignals::default();
        store
            .write_score("owner_1", "chunk_001", &signals, 0.5, false, None)
            .unwrap();

        store
            .set_llm_importance("owner_1", "chunk_001", 0.9, Some("very important"))
            .unwrap();

        let (importance, reason) = store
            .get_llm_importance("owner_1", "chunk_001")
            .unwrap()
            .unwrap();
        assert!((importance - 0.9).abs() < 0.01);
        assert_eq!(reason, Some("very important".to_string()));
    }

    #[test]
    fn test_owner_isolation() {
        let store = setup();
        let signals = ScoreSignals::default();
        store
            .write_score("owner_1", "chunk_001", &signals, 0.5, false, None)
            .unwrap();

        assert!(store.get_score("owner_2", "chunk_001").unwrap().is_none());
    }
}
