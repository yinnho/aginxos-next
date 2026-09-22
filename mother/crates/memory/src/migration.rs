//! SQLite schema creation and migration.
//!
//! Creates all tables needed by the memory substrate on first boot.

use rusqlite::Connection;

/// Current schema version.
const SCHEMA_VERSION: u32 = 33;

/// Run all migrations to bring the database up to date.
///
/// Each migration is wrapped in its own transaction so that a partial failure
/// doesn't leave the database in an inconsistent state.
pub fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    let current_version = get_schema_version(conn);

    type MigrateFn = fn(&Connection) -> Result<(), rusqlite::Error>;
    let migrations: Vec<(u32, MigrateFn)> = vec![
        (1, migrate_v1),
        (2, migrate_v2),
        (3, migrate_v3),
        (4, migrate_v4),
        (5, migrate_v5),
        (6, migrate_v6),
        (7, migrate_v7),
        (8, migrate_v8),
        (9, migrate_v9),
        (10, migrate_v10),
        (11, migrate_v11),
        (12, migrate_v12),
        (13, migrate_v13),
        (14, migrate_v14),
        (15, migrate_v15),
        (16, migrate_v16),
        (17, migrate_v17),
        (18, migrate_v18),
        (19, migrate_v19),
        (20, migrate_v20),
        (21, migrate_v21),
        (22, migrate_v22),
        (23, migrate_v23),
        (24, migrate_v24),
        (25, migrate_v25),
        (26, migrate_v26),
        (27, migrate_v27),
        (28, migrate_v28),
        (29, migrate_v29),
        (30, migrate_v30),
        (31, migrate_v31),
        (32, migrate_v32),
        (33, migrate_v33),
    ];

    for (version, migrate_fn) in &migrations {
        if current_version < *version {
            conn.execute_batch("BEGIN")?;
            if let Err(e) = migrate_fn(conn) {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
            conn.execute_batch("COMMIT")?;
        }
    }

    set_schema_version(conn, SCHEMA_VERSION)?;

    // Clean up old kv_history entries (older than 30 days)
    match conn.execute(
        "DELETE FROM kv_history WHERE archived_at < datetime('now', '-30 days')",
        [],
    ) {
        Ok(deleted) => {
            if deleted > 0 {
                tracing::info!(
                    deleted,
                    "kv_history cleanup: removed entries older than 30 days"
                );
            }
        }
        Err(e) => {
            // Non-fatal: old data just takes space
            tracing::warn!("kv_history cleanup warning: {e}");
        }
    }

    Ok(())
}

/// Get the current schema version from the database.
fn get_schema_version(conn: &Connection) -> u32 {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0)
}

/// Check if a column exists in a table (SQLite has no ADD COLUMN IF NOT EXISTS).
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({})", table);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    let names: Vec<String> = rows.filter_map(|r| r.ok()).collect();
    names.iter().any(|n| n == column)
}

/// Set the schema version in the database.
fn set_schema_version(conn: &Connection, version: u32) -> Result<(), rusqlite::Error> {
    conn.pragma_update(None, "user_version", version)
}

/// Version 1: Create all core tables.
fn migrate_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        -- Agent registry
        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            manifest BLOB NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Session history
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            messages BLOB NOT NULL,
            context_window_tokens INTEGER DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Event log
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            source_agent TEXT NOT NULL,
            target TEXT NOT NULL,
            payload BLOB NOT NULL,
            timestamp TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
        CREATE INDEX IF NOT EXISTS idx_events_source ON events(source_agent);

        -- Key-value store (per-agent)
        CREATE TABLE IF NOT EXISTS kv_store (
            agent_id TEXT NOT NULL,
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (agent_id, key)
        );

        -- Task queue
        CREATE TABLE IF NOT EXISTS task_queue (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            task_type TEXT NOT NULL,
            payload BLOB NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            priority INTEGER NOT NULL DEFAULT 0,
            scheduled_at TEXT,
            created_at TEXT NOT NULL,
            completed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_task_status_priority ON task_queue(status, priority DESC);

        -- Semantic memories
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            content TEXT NOT NULL,
            source TEXT NOT NULL,
            scope TEXT NOT NULL DEFAULT 'episodic',
            confidence REAL NOT NULL DEFAULT 1.0,
            metadata TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            accessed_at TEXT NOT NULL,
            access_count INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_memories_agent ON memories(agent_id);
        CREATE INDEX IF NOT EXISTS idx_memories_scope ON memories(scope);

        -- Knowledge graph entities
        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY,
            entity_type TEXT NOT NULL,
            name TEXT NOT NULL,
            properties TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Knowledge graph relations
        CREATE TABLE IF NOT EXISTS relations (
            id TEXT PRIMARY KEY,
            source_entity TEXT NOT NULL,
            relation_type TEXT NOT NULL,
            target_entity TEXT NOT NULL,
            properties TEXT NOT NULL DEFAULT '{}',
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_entity);
        CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_entity);
        CREATE INDEX IF NOT EXISTS idx_relations_type ON relations(relation_type);

        -- Migration tracking
        CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL,
            description TEXT
        );

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (1, datetime('now'), 'Initial schema');
        ",
    )?;
    Ok(())
}

/// Version 2: Add collaboration columns to task_queue for agent task delegation.
fn migrate_v2(conn: &Connection) -> Result<(), rusqlite::Error> {
    // SQLite requires one ALTER TABLE per statement; check before adding
    let cols = [
        ("title", "TEXT DEFAULT ''"),
        ("description", "TEXT DEFAULT ''"),
        ("assigned_to", "TEXT DEFAULT ''"),
        ("created_by", "TEXT DEFAULT ''"),
        ("result", "TEXT DEFAULT ''"),
    ];
    for (name, typedef) in &cols {
        if !column_exists(conn, "task_queue", name) {
            conn.execute(
                &format!("ALTER TABLE task_queue ADD COLUMN {} {}", name, typedef),
                [],
            )?;
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (2, datetime('now'), 'Add collaboration columns to task_queue')",
        [],
    )?;

    Ok(())
}

/// Version 3: Add embedding column to memories table for vector search.
fn migrate_v3(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "memories", "embedding") {
        conn.execute(
            "ALTER TABLE memories ADD COLUMN embedding BLOB DEFAULT NULL",
            [],
        )?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (3, datetime('now'), 'Add embedding column to memories')",
        [],
    )?;
    Ok(())
}

/// Version 4: Add usage_events table for cost tracking and metering.
fn migrate_v4(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS usage_events (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            tool_calls INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_usage_agent_time ON usage_events(agent_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage_events(timestamp);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (4, datetime('now'), 'Add usage_events table for cost tracking');
        ",
    )?;
    Ok(())
}

/// Version 5: Add canonical_sessions table for cross-channel persistent memory.
fn migrate_v5(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS canonical_sessions (
            agent_id TEXT PRIMARY KEY,
            messages BLOB NOT NULL,
            compaction_cursor INTEGER NOT NULL DEFAULT 0,
            compacted_summary TEXT,
            updated_at TEXT NOT NULL
        );

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (5, datetime('now'), 'Add canonical_sessions for cross-channel memory');
        ",
    )?;
    Ok(())
}

/// Version 6: Add label column to sessions table.
fn migrate_v6(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Check if column already exists before ALTER (SQLite has no ADD COLUMN IF NOT EXISTS)
    if !column_exists(conn, "sessions", "label") {
        conn.execute("ALTER TABLE sessions ADD COLUMN label TEXT", [])?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (6, datetime('now'), 'Add label column to sessions for human-readable labels')",
        [],
    )?;
    Ok(())
}

/// Version 7: Add paired_devices table for device pairing persistence.
fn migrate_v7(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS paired_devices (
            device_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            platform TEXT NOT NULL,
            paired_at TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            push_token TEXT
        );

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (7, datetime('now'), 'Add paired_devices table for device pairing');
        ",
    )?;
    Ok(())
}

/// Version 8: Add audit_entries table for persistent Merkle audit trail.
fn migrate_v8(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS audit_entries (
            seq INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            action TEXT NOT NULL,
            detail TEXT NOT NULL,
            outcome TEXT NOT NULL,
            prev_hash TEXT NOT NULL,
            hash TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_audit_agent ON audit_entries(agent_id);
        CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_entries(timestamp);
        CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_entries(action);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (8, datetime('now'), 'Add audit_entries table for persistent Merkle audit trail');
        ",
    )?;
    Ok(())
}

/// Version 9: Add kv_history table for memory immutability.
///
/// Before any overwrite or delete in kv_store, the old value is archived here.
/// This ensures no memory is ever truly lost.
fn migrate_v9(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS kv_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            version INTEGER NOT NULL,
            archived_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_kv_history_agent_key ON kv_history(agent_id, key);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (9, datetime('now'), 'Add kv_history table for memory immutability');
        ",
    )?;
    Ok(())
}

/// Version 10: Multi-tenant support.
///
/// - Creates `tenants` table for tenant/user management.
/// - Adds nullable `tenant_id` column to all data tables for tenant isolation.
/// - Existing rows get `tenant_id = NULL` (global/admin scope).
fn migrate_v10(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Create tenants table (serves as both tenant and user table).
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS tenants (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'tenant',
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )?;

    // Add tenant_id to all data tables (nullable for backward compatibility).
    let tables = [
        "agents",
        "sessions",
        "events",
        "kv_store",
        "task_queue",
        "memories",
        "entities",
        "relations",
        "usage_events",
        "canonical_sessions",
        "audit_entries",
        "kv_history",
    ];

    for table in &tables {
        if !column_exists(conn, table, "tenant_id") {
            let sql = format!("ALTER TABLE {table} ADD COLUMN tenant_id TEXT DEFAULT NULL");
            conn.execute_batch(&sql)?;
        }
        let idx = format!("CREATE INDEX IF NOT EXISTS idx_{table}_tenant ON {table}(tenant_id)");
        conn.execute_batch(&idx)?;
    }

    conn.execute_batch(
        "
        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (10, datetime('now'), 'Multi-tenant: tenants table + tenant_id on all data tables');
        ",
    )?;
    Ok(())
}

/// Version 11: Add invite tracking table for share-page referral analytics.
fn migrate_v11(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS invites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            inviter_fp TEXT NOT NULL,
            invitee_tenant_id TEXT,
            invited_at TEXT NOT NULL,
            converted_at TEXT,
            source_platform TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_invites_inviter ON invites(inviter_fp);
        CREATE INDEX IF NOT EXISTS idx_invites_invitee ON invites(invitee_tenant_id);
        CREATE INDEX IF NOT EXISTS idx_invites_invited_at ON invites(invited_at);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (11, datetime('now'), 'Add invite tracking table for share-page referral analytics');
        ",
    )?;
    Ok(())
}

/// Version 12: Remove tenant layer — drop `tenants` and `canonical_sessions` tables.
///
/// Unused `tenant_id` columns remain in other tables (SQLite cannot DROP COLUMN portably).
fn migrate_v12(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        DROP TABLE IF EXISTS tenants;
        DROP TABLE IF EXISTS canonical_sessions;

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (12, datetime('now'), 'Remove tenant layer: drop tenants and canonical_sessions tables');
        ",
    )?;
    Ok(())
}

/// Version 13: Per-user memory isolation — add `sender_id` to kv_store and kv_history.
///
/// The composite primary key becomes (agent_id, sender_id, key) so that
/// each user's memory is isolated within a shared clone.
/// Existing rows get `sender_id = ''` (system/internal context).
fn migrate_v13(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Add sender_id column if missing
    if !column_exists(conn, "kv_store", "sender_id") {
        conn.execute_batch("ALTER TABLE kv_store ADD COLUMN sender_id TEXT NOT NULL DEFAULT ''")?;
    }
    if !column_exists(conn, "kv_history", "sender_id") {
        conn.execute_batch("ALTER TABLE kv_history ADD COLUMN sender_id TEXT NOT NULL DEFAULT ''")?;
    }

    // Recreate kv_store with new composite primary key
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS kv_store_v13 (
            agent_id TEXT NOT NULL,
            sender_id TEXT NOT NULL DEFAULT '',
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (agent_id, sender_id, key)
        );
        INSERT OR IGNORE INTO kv_store_v13 (agent_id, sender_id, key, value, version, updated_at)
            SELECT agent_id, COALESCE(sender_id, ''), key, value, version, updated_at FROM kv_store;
        DROP TABLE kv_store;
        ALTER TABLE kv_store_v13 RENAME TO kv_store;
        ",
    )?;

    // Recreate kv_history with sender_id
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS kv_history_v13 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            sender_id TEXT NOT NULL DEFAULT '',
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            version INTEGER NOT NULL,
            archived_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO kv_history_v13 (agent_id, sender_id, key, value, version, archived_at)
            SELECT agent_id, COALESCE(sender_id, ''), key, value, version, archived_at FROM kv_history;
        DROP TABLE kv_history;
        ALTER TABLE kv_history_v13 RENAME TO kv_history;
        CREATE INDEX IF NOT EXISTS idx_kv_history_agent_key ON kv_history(agent_id, key);
        ",
    )?;

    conn.execute_batch(
        "
        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (13, datetime('now'), 'Per-user memory: add sender_id to kv_store/kv_history');
        ",
    )?;
    Ok(())
}

/// Version 14: Owner/user data model — split `sender_id` into `owner_id` + `user_id`.
///
/// The composite primary key becomes (agent_id, owner_id, user_id, key).
/// Existing rows get `owner_id = user_id = sender_id` (owner is the same as user).
fn migrate_v14(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Recreate kv_store with owner_id + user_id
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS kv_store_v14 (
            agent_id TEXT NOT NULL,
            owner_id TEXT NOT NULL DEFAULT '',
            user_id  TEXT NOT NULL DEFAULT '',
            key      TEXT NOT NULL,
            value    BLOB NOT NULL,
            version  INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (agent_id, owner_id, user_id, key)
        );
        INSERT OR IGNORE INTO kv_store_v14 (agent_id, owner_id, user_id, key, value, version, updated_at)
            SELECT agent_id, COALESCE(sender_id, ''), COALESCE(sender_id, ''), key, value, version, updated_at FROM kv_store;
        DROP TABLE kv_store;
        ALTER TABLE kv_store_v14 RENAME TO kv_store;
        ",
    )?;

    // Recreate kv_history with owner_id + user_id
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS kv_history_v14 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            owner_id TEXT NOT NULL DEFAULT '',
            user_id  TEXT NOT NULL DEFAULT '',
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            version INTEGER NOT NULL,
            archived_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO kv_history_v14 (agent_id, owner_id, user_id, key, value, version, archived_at)
            SELECT agent_id, COALESCE(sender_id, ''), COALESCE(sender_id, ''), key, value, version, archived_at FROM kv_history;
        DROP TABLE kv_history;
        ALTER TABLE kv_history_v14 RENAME TO kv_history;
        CREATE INDEX IF NOT EXISTS idx_kv_history_agent_key ON kv_history(agent_id, key);
        ",
    )?;

    conn.execute_batch(
        "
        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (14, datetime('now'), 'Owner/user model: split sender_id into owner_id + user_id in kv_store/kv_history');
        ",
    )?;
    Ok(())
}

fn migrate_v15(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "sessions", "active_toolsets") {
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN active_toolsets TEXT NOT NULL DEFAULT '[]';
            ",
        )?;
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![15, "Session: add active_toolsets for toolset on-demand loading"],
    )?;
    Ok(())
}

/// Version 16: Add tables for cron delivery routing.
/// - `sender_channels`: tracks which channel a sender last used (for cron delivery)
/// - `pending_notifications`: buffers cron notifications for channels that
///   don't support proactive push, until the user sends an inbound message
fn migrate_v16(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sender_channels (
            sender_id TEXT PRIMARY KEY,
            channel_type TEXT NOT NULL,
            bot_id TEXT NOT NULL,
            last_seen_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS pending_notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            sender_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            message TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'cron',
            created_at INTEGER NOT NULL,
            delivered_at INTEGER NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_pending_sender_undelivered
            ON pending_notifications(sender_id, delivered_at);
        CREATE INDEX IF NOT EXISTS idx_pending_expires
            ON pending_notifications(expires_at);
        ",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![16, "Cron delivery: sender_channels + pending_notifications"],
    )?;
    Ok(())
}

fn migrate_v17(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Tree memory system: 9 new tables for hierarchical memory with multi-tenancy.
    // Every table includes owner_id as the leading partition key.
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS mem_tree_chunks (
            id                     TEXT PRIMARY KEY,
            owner_id               TEXT NOT NULL,
            agent_id               TEXT NOT NULL,
            source_kind            TEXT NOT NULL,
            source_id              TEXT NOT NULL,
            source_ref             TEXT,
            timestamp_ms           INTEGER NOT NULL,
            time_range_start_ms    INTEGER NOT NULL,
            time_range_end_ms      INTEGER NOT NULL,
            tags_json              TEXT NOT NULL DEFAULT '[]',
            content                TEXT NOT NULL,
            token_count            INTEGER NOT NULL,
            seq_in_source          INTEGER NOT NULL,
            partial_message        INTEGER NOT NULL DEFAULT 0,
            lifecycle_status       TEXT NOT NULL DEFAULT 'admitted',
            created_at_ms          INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_owner
            ON mem_tree_chunks(owner_id);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_owner_source
            ON mem_tree_chunks(owner_id, source_kind, source_id);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_owner_timestamp
            ON mem_tree_chunks(owner_id, timestamp_ms);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_owner_lifecycle
            ON mem_tree_chunks(owner_id, lifecycle_status);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_source_seq
            ON mem_tree_chunks(owner_id, source_kind, source_id, seq_in_source);

        CREATE TABLE IF NOT EXISTS mem_tree_score (
            chunk_id               TEXT PRIMARY KEY,
            owner_id               TEXT NOT NULL,
            total                  REAL NOT NULL,
            token_count_signal     REAL NOT NULL,
            unique_words_signal    REAL NOT NULL,
            metadata_weight        REAL NOT NULL,
            source_weight          REAL NOT NULL,
            interaction_weight     REAL NOT NULL,
            entity_density         REAL NOT NULL,
            llm_importance         REAL NOT NULL DEFAULT 0.0,
            llm_importance_reason  TEXT,
            dropped                INTEGER NOT NULL DEFAULT 0,
            reason                 TEXT,
            computed_at_ms         INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_score_owner_total
            ON mem_tree_score(owner_id, total);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_score_owner_dropped
            ON mem_tree_score(owner_id, dropped);

        CREATE TABLE IF NOT EXISTS mem_tree_entity_index (
            entity_id              TEXT NOT NULL,
            node_id                TEXT NOT NULL,
            node_kind              TEXT NOT NULL,
            owner_id               TEXT NOT NULL,
            entity_kind            TEXT NOT NULL,
            surface                TEXT NOT NULL,
            score                  REAL NOT NULL,
            timestamp_ms           INTEGER NOT NULL,
            tree_id                TEXT,
            PRIMARY KEY (owner_id, entity_id, node_id)
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_owner_entity
            ON mem_tree_entity_index(owner_id, entity_id);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_owner_node
            ON mem_tree_entity_index(owner_id, node_id);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_owner_timestamp
            ON mem_tree_entity_index(owner_id, timestamp_ms);

        CREATE TABLE IF NOT EXISTS mem_tree_trees (
            id                     TEXT PRIMARY KEY,
            owner_id               TEXT NOT NULL,
            kind                   TEXT NOT NULL,
            scope                  TEXT NOT NULL,
            root_id                TEXT,
            max_level              INTEGER NOT NULL DEFAULT 0,
            status                 TEXT NOT NULL DEFAULT 'active',
            created_at_ms          INTEGER NOT NULL,
            last_sealed_at_ms      INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_mem_tree_trees_owner_kind_scope
            ON mem_tree_trees(owner_id, kind, scope);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_trees_owner_status
            ON mem_tree_trees(owner_id, status);

        CREATE TABLE IF NOT EXISTS mem_tree_summaries (
            id                     TEXT PRIMARY KEY,
            owner_id               TEXT NOT NULL,
            tree_id                TEXT NOT NULL,
            tree_kind              TEXT NOT NULL,
            level                  INTEGER NOT NULL,
            parent_id              TEXT,
            child_ids_json         TEXT NOT NULL DEFAULT '[]',
            content                TEXT NOT NULL,
            token_count            INTEGER NOT NULL,
            entities_json          TEXT NOT NULL DEFAULT '[]',
            topics_json            TEXT NOT NULL DEFAULT '[]',
            time_range_start_ms    INTEGER NOT NULL,
            time_range_end_ms      INTEGER NOT NULL,
            score                  REAL NOT NULL DEFAULT 0.0,
            sealed_at_ms           INTEGER NOT NULL,
            deleted                INTEGER NOT NULL DEFAULT 0,
            embedding              BLOB DEFAULT NULL,
            FOREIGN KEY (tree_id) REFERENCES mem_tree_trees(id)
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_owner_tree_level
            ON mem_tree_summaries(owner_id, tree_id, level);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_owner_parent
            ON mem_tree_summaries(owner_id, parent_id);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_owner_sealed_at
            ON mem_tree_summaries(owner_id, sealed_at_ms);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_owner_deleted
            ON mem_tree_summaries(owner_id, deleted);

        CREATE TABLE IF NOT EXISTS mem_tree_buffers (
            tree_id                TEXT NOT NULL,
            level                  INTEGER NOT NULL,
            owner_id               TEXT NOT NULL,
            item_ids_json          TEXT NOT NULL DEFAULT '[]',
            token_sum              INTEGER NOT NULL DEFAULT 0,
            oldest_at_ms           INTEGER,
            updated_at_ms          INTEGER NOT NULL,
            PRIMARY KEY (tree_id, level),
            FOREIGN KEY (tree_id) REFERENCES mem_tree_trees(id)
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_buffers_owner_oldest
            ON mem_tree_buffers(owner_id, oldest_at_ms);

        CREATE TABLE IF NOT EXISTS mem_tree_entity_hotness (
            entity_id              TEXT NOT NULL,
            owner_id               TEXT NOT NULL,
            mention_count_30d      INTEGER NOT NULL DEFAULT 0,
            distinct_sources       INTEGER NOT NULL DEFAULT 0,
            last_seen_ms           INTEGER,
            query_hits_30d         INTEGER NOT NULL DEFAULT 0,
            graph_centrality       REAL,
            ingests_since_check    INTEGER NOT NULL DEFAULT 0,
            last_hotness           REAL,
            last_updated_ms        INTEGER NOT NULL,
            PRIMARY KEY (owner_id, entity_id)
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_hotness_owner_score
            ON mem_tree_entity_hotness(owner_id, last_hotness);

        CREATE TABLE IF NOT EXISTS mem_tree_jobs (
            id                     TEXT PRIMARY KEY,
            owner_id               TEXT NOT NULL,
            kind                   TEXT NOT NULL,
            payload_json           TEXT NOT NULL,
            dedupe_key             TEXT,
            status                 TEXT NOT NULL DEFAULT 'ready',
            attempts               INTEGER NOT NULL DEFAULT 0,
            max_attempts           INTEGER NOT NULL DEFAULT 5,
            available_at_ms        INTEGER NOT NULL,
            locked_until_ms        INTEGER,
            last_error             TEXT,
            created_at_ms          INTEGER NOT NULL,
            started_at_ms          INTEGER,
            completed_at_ms        INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_mem_tree_jobs_owner_ready
            ON mem_tree_jobs(owner_id, status, available_at_ms);
        CREATE INDEX IF NOT EXISTS idx_mem_tree_jobs_owner_kind
            ON mem_tree_jobs(owner_id, kind);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_mem_tree_jobs_owner_dedupe_active
            ON mem_tree_jobs(owner_id, dedupe_key)
            WHERE dedupe_key IS NOT NULL AND status IN ('ready', 'running');

        CREATE TABLE IF NOT EXISTS mem_tree_ingested_sources (
            source_kind            TEXT NOT NULL,
            source_id              TEXT NOT NULL,
            owner_id               TEXT NOT NULL,
            ingested_at_ms         INTEGER NOT NULL,
            PRIMARY KEY (owner_id, source_kind, source_id)
        );
        ",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![17, "Tree memory: 9 tables for hierarchical memory with owner_id partitioning"],
    )?;
    Ok(())
}

/// Version 18: Add active_skill_name column to sessions.
fn migrate_v18(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "sessions", "active_skill_name") {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN active_skill_name TEXT DEFAULT NULL")?;
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![18, "Session: add active_skill_name for skill write-back tracking"],
    )?;
    Ok(())
}

/// Version 19: Add session_id and identity columns to agents table (moved from save_agent hot-path).
fn migrate_v19(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "agents", "session_id") {
        conn.execute_batch("ALTER TABLE agents ADD COLUMN session_id TEXT DEFAULT ''")?;
    }
    if !column_exists(conn, "agents", "identity") {
        conn.execute_batch("ALTER TABLE agents ADD COLUMN identity TEXT DEFAULT '{}'")?;
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![19, "Agents: add session_id and identity columns (from hot-path to migration)"],
    )?;
    Ok(())
}

/// Version 20: Add turn_summaries column to sessions for turn-level context layering.
fn migrate_v20(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "sessions", "turn_summaries") {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN turn_summaries BLOB DEFAULT NULL")?;
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![20, "Sessions: add turn_summaries column for turn-level context layering"],
    )?;
    Ok(())
}

/// Version 21: Add cron_jobs table for DB-backed cron persistence.
/// Migrates existing cron_jobs.json data if present.
fn migrate_v21(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS cron_jobs (
            id          TEXT PRIMARY KEY,
            agent_id    TEXT NOT NULL,
            owner_id    TEXT,
            sender_id   TEXT,
            name        TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1,
            schedule    TEXT NOT NULL,
            action      TEXT NOT NULL,
            delivery    TEXT NOT NULL DEFAULT '{\"kind\":\"none\"}',
            one_shot    INTEGER NOT NULL DEFAULT 0,
            last_status TEXT,
            consecutive_errors INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL,
            last_run    TEXT,
            next_run    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_cron_agent ON cron_jobs(agent_id);
        ",
    )?;

    // Migrate existing cron_jobs.json if present
    let home_dir = carrier_types::config::home_dir();
    let json_path = home_dir.join("cron_jobs.json");
    if json_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&json_path) {
            if let Ok(metas) = serde_json::from_str::<Vec<serde_json::Value>>(&data) {
                for meta_val in &metas {
                    if let Some(job_val) = meta_val.get("job") {
                        let id = job_val.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let agent_id = job_val
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let owner_id = job_val.get("owner_id").and_then(|v| v.as_str());
                        let sender_id = job_val.get("sender_id").and_then(|v| v.as_str());
                        let name = job_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let enabled = job_val
                            .get("enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        let schedule = job_val
                            .get("schedule")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let action = job_val
                            .get("action")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let delivery = job_val
                            .get("delivery")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{\"kind\":\"none\"}".to_string());
                        let one_shot = meta_val
                            .get("one_shot")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let last_status = meta_val.get("last_status").and_then(|v| v.as_str());
                        let consecutive_errors = meta_val
                            .get("consecutive_errors")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let created_at = job_val
                            .get("created_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let last_run = job_val.get("last_run").and_then(|v| v.as_str());
                        let next_run = job_val.get("next_run").and_then(|v| v.as_str());

                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO cron_jobs (id, agent_id, owner_id, sender_id, name, enabled, schedule, action, delivery, one_shot, last_status, consecutive_errors, created_at, last_run, next_run) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                            rusqlite::params![id, agent_id, owner_id, sender_id, name, enabled as i32, schedule, action, delivery, one_shot as i32, last_status, consecutive_errors as i32, created_at, last_run, next_run],
                        );
                    }
                }
            }
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![21, "Cron jobs: migrate cron_jobs.json to cron_jobs table"],
    )?;
    Ok(())
}

/// Version 22: Add weixin_sessions table for DB-backed iLink session persistence.
/// Migrates existing senders/*/session.json data if present.
fn migrate_v22(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS weixin_sessions (
            user_id       TEXT PRIMARY KEY,
            channel       TEXT NOT NULL DEFAULT 'weixin',
            sender_key    TEXT NOT NULL DEFAULT 'openid',
            bot_id        TEXT NOT NULL,
            bot_token     TEXT NOT NULL,
            baseurl       TEXT NOT NULL,
            ilink_bot_id  TEXT NOT NULL,
            expires_at    INTEGER NOT NULL,
            bind_agent    TEXT,
            context_tokens TEXT NOT NULL DEFAULT '{}',
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )?;

    // Migrate existing senders/*/session.json files
    let home_dir = carrier_types::config::home_dir();
    let senders_dir = home_dir.join("senders");
    if senders_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&senders_dir) {
            for entry in entries.flatten() {
                let session_path = entry.path().join("session.json");
                if !session_path.exists() {
                    continue;
                }
                let data = match std::fs::read_to_string(&session_path) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let json: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                // Only migrate weixin sessions
                if json.get("channel").and_then(|v| v.as_str()) != Some("weixin") {
                    continue;
                }
                let user_id = match json.get("user_id").and_then(|v| v.as_str()) {
                    Some(uid) if !uid.is_empty() => uid,
                    _ => continue,
                };
                let bot_id = json.get("bot_id").and_then(|v| v.as_str()).unwrap_or("");
                let bot_token = json.get("bot_token").and_then(|v| v.as_str()).unwrap_or("");
                let baseurl = json.get("baseurl").and_then(|v| v.as_str()).unwrap_or("");
                let ilink_bot_id = json
                    .get("ilink_bot_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let expires_at = json.get("expires_at").and_then(|v| v.as_i64()).unwrap_or(0);
                let bind_agent = json.get("bind_agent").and_then(|v| v.as_str());
                let context_tokens = json
                    .get("context_tokens")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());

                let _ = conn.execute(
                    "INSERT OR IGNORE INTO weixin_sessions (user_id, channel, sender_key, bot_id, bot_token, baseurl, ilink_bot_id, expires_at, bind_agent, context_tokens) \
                     VALUES (?1, 'weixin', 'openid', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![user_id, bot_id, bot_token, baseurl, ilink_bot_id, expires_at, bind_agent, context_tokens],
                );
            }
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![22, "WeChat iLink: migrate senders/*/session.json to weixin_sessions table"],
    )?;
    Ok(())
}

/// Version 23: Add notify_routes table for DB-backed notification routing.
/// Migrates existing notify_routes.json data if present.
fn migrate_v23(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS notify_routes (
            name       TEXT PRIMARY KEY,
            channel    TEXT NOT NULL,
            bot_id     TEXT NOT NULL DEFAULT '',
            user_id    TEXT NOT NULL DEFAULT '',
            prefix     TEXT,
            recipients TEXT
        );
        ",
    )?;

    // Migrate existing notify_routes.json if present
    let home_dir = carrier_types::config::home_dir();
    let json_path = home_dir.join("notify_routes.json");
    if json_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&json_path) {
            if let Ok(routes) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(map) = routes.as_object() {
                    for (name, val) in map {
                        let channel = val.get("channel").and_then(|v| v.as_str()).unwrap_or("");
                        let bot_id = val.get("bot_id").and_then(|v| v.as_str()).unwrap_or("");
                        let user_id = val.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
                        let prefix = val.get("prefix").and_then(|v| v.as_str());
                        let recipients = val.get("recipients").and_then(|v| v.as_str());
                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO notify_routes (name, channel, bot_id, user_id, prefix, recipients) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            rusqlite::params![name, channel, bot_id, user_id, prefix, recipients],
                        );
                    }
                }
            }
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![23, "Notify routes: migrate notify_routes.json to notify_routes table"],
    )?;
    Ok(())
}

/// Version 24: flow_runs table for multi-step flow execution state.
///
/// Stores `run_flow` execution state (completed step outputs, the waiting
/// `user_input` step, status). In stage 2 incremental B it serves as run
/// history/audit; the `waiting_at`/`map_context` columns are populated once
/// `user_input` suspend/resume lands (stage D). Schema is defined in full now
/// so a later stage doesn't need another migration.
fn migrate_v24(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS flow_runs (
            run_id          TEXT PRIMARY KEY,
            session_id      TEXT NOT NULL,
            agent_id        TEXT NOT NULL,
            sender_id       TEXT NOT NULL DEFAULT '',
            flow_name       TEXT NOT NULL,
            input           TEXT NOT NULL DEFAULT '{}',
            completed_steps TEXT NOT NULL DEFAULT '{}',
            waiting_at      TEXT,
            map_context     TEXT,
            status          TEXT NOT NULL DEFAULT 'waiting',
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_flow_runs_pending
            ON flow_runs(sender_id, agent_id) WHERE status = 'waiting';
        ",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![24, "flow_runs table for multi-step flow execution state"],
    )?;
    Ok(())
}

/// Version 25: add `expires_at` to `flow_runs` for `user_input` timeouts.
///
/// When a flow suspends at a `user_input` step, `set_waiting` records the
/// deadline here; the resume check and a background tick mark runs past it
/// `timed_out`. Mirrors the `cron_delivery.expires_at` precedent.
fn migrate_v25(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("ALTER TABLE flow_runs ADD COLUMN expires_at TEXT")?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![25, "flow_runs.expires_at for user_input timeouts"],
    )?;
    Ok(())
}

/// Drop dead tables that have no readers/writers anywhere in the codebase.
///
/// `memories`, `entities`, `relations` (v1), `canonical_sessions` (v5), and
/// `paired_devices` (v7) were created by early experiments but nothing reads or
/// writes them today (verified: zero references outside migration.rs). They only
/// occupy space and PRAGMA scan cost. `IF EXISTS` makes this safe for fresh
/// installs that never had them and idempotent for re-runs.
fn migrate_v26(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS memories;
         DROP TABLE IF EXISTS entities;
         DROP TABLE IF EXISTS relations;
         DROP TABLE IF EXISTS canonical_sessions;
         DROP TABLE IF EXISTS paired_devices;",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![26, "drop dead tables (memories/entities/relations/canonical_sessions/paired_devices)"],
    )?;
    Ok(())
}

/// Version 27: Per-user isolation for the memory tree.
///
/// Adds a `user_id` column to the four content-bearing mem_tree tables
/// (chunks, trees, summaries, entity_index). `''` — the column default — means
/// owner-shared (visible to every user under that owner). New ingests write the
/// real sender_id (see `agent_loop/end_turn.rs`), and user-scoped queries filter
/// with `(user_id = ? OR user_id = '')`: new data is isolated to its user while
/// legacy rows (pre-v27, `user_id = ''`) stay owner-shared until they age out,
/// so there is no recall regression. Hotness/buffers/ingested_sources are
/// owner-aggregates by design and are intentionally NOT given a `user_id`.
///
/// We deliberately do NOT backfill `user_id` from `source_id`: the
/// source_id→sender_id mapping is channel-specific (one colon for some
/// channels, two for wechat-OA) and a wrong derivation would make legacy rows
/// invisible instead of shared. The fallback clause preserves legacy recall;
/// new data is correctly tagged going forward.
fn migrate_v27(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ALTER TABLE ADD COLUMN with a constant DEFAULT is metadata-only in SQLite
    // (no table rewrite), so this is cheap even on large tables.
    for table in &[
        "mem_tree_chunks",
        "mem_tree_trees",
        "mem_tree_summaries",
        "mem_tree_entity_index",
    ] {
        if !column_exists(conn, table, "user_id") {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN user_id TEXT NOT NULL DEFAULT ''"
            ))?;
        }
    }

    // Per-user filter indexes for the hot query paths.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_owner_user
            ON mem_tree_chunks(owner_id, user_id);
         CREATE INDEX IF NOT EXISTS idx_mem_tree_trees_owner_user_kind
            ON mem_tree_trees(owner_id, user_id, kind);
         CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_owner_user_tree
            ON mem_tree_summaries(owner_id, user_id, tree_id);
         CREATE INDEX IF NOT EXISTS idx_mem_tree_entity_index_owner_user
            ON mem_tree_entity_index(owner_id, user_id);",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![27, "per-user tree isolation: user_id on chunks/trees/summaries/entity_index"],
    )?;
    Ok(())
}

/// Version 28: Automation rules - per-app/channel "trigger -> fixed action"
/// rules matched on inbound events (subscribe/keyword) and executed by the
/// channel layer WITHOUT routing to the agent LLM. Configured by admins via
/// agent tools. `task_payload` is a `ContentDescriptor`-shaped JSON object.
fn migrate_v28(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS automation_rules (
            id           TEXT PRIMARY KEY,
            app_id       TEXT NOT NULL,
            channel      TEXT NOT NULL DEFAULT 'weixin-oa',
            name         TEXT NOT NULL,
            enabled      INTEGER NOT NULL DEFAULT 1,
            priority     INTEGER NOT NULL DEFAULT 0,
            trigger_kind TEXT NOT NULL,
            trigger_data TEXT NOT NULL DEFAULT '',
            task_kind    TEXT NOT NULL,
            task_payload TEXT NOT NULL,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_automation_rules_app
            ON automation_rules(channel, app_id, enabled, priority);",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![28, "Automation rules: per-app inbound fixed-reply rules (subscribe/keyword -> push)"],
    )?;
    Ok(())
}

/// Version 29: Unified push — add `target` column to automation_rules.
/// Existing notify_admin rules get target='admins'; all others default
/// to 'current' (the triggering user). Metadata-only ALTER (no rewrite).
fn migrate_v29(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "automation_rules", "target") {
        conn.execute_batch(
            "ALTER TABLE automation_rules ADD COLUMN target TEXT NOT NULL DEFAULT 'current';",
        )?;
    }
    // Backfill: legacy notify_admin rules -> target='admins'.
    conn.execute(
        "UPDATE automation_rules SET target='admins' WHERE task_kind='notify_admin' AND target='current';",
        [],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![29, "Unified push: target column on automation_rules"],
    )?;
    Ok(())
}

/// Version 30: chained-pipeline identity column on cron_jobs (Plan A of
/// broken-chain monitoring — `CronJob.chain: Option<ChainMeta>`). Nullable:
/// existing jobs are not part of a declared chain and get no detection.
fn migrate_v30(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "cron_jobs", "chain") {
        conn.execute_batch("ALTER TABLE cron_jobs ADD COLUMN chain TEXT;")?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![30, "Chained cron: chain (ChainMeta JSON) column on cron_jobs"],
    )?;
    Ok(())
}

/// Version 31: channel followers ledger (automation Phase 2). Records
/// follow/unfollow lifecycle + `last_seen` (any inbound refresh) per
/// `(channel, app_id, openid)`. `last_seen` bounds the deliverable audience
/// for scheduled pushes (OA customer-service 48h window) and feeds
/// `FollowerReport` growth digests.
fn migrate_v31(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS channel_followers (
            channel TEXT NOT NULL,
            app_id TEXT NOT NULL,
            openid TEXT NOT NULL,
            unionid TEXT,
            followed_at TEXT NOT NULL,
            unfollowed_at TEXT,
            last_seen_at TEXT NOT NULL,
            last_scene TEXT,
            PRIMARY KEY (channel, app_id, openid)
        );
        CREATE INDEX IF NOT EXISTS idx_followers_last_seen
            ON channel_followers(channel, app_id, last_seen_at);",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![31, "Followers ledger: channel_followers table (automation Phase 2)"],
    )?;
    Ok(())
}

/// Version 32: chain-resume ledger (断链自动接续). Per-`(chain_id, step)`
/// auto-resume attempt budget for chained one-shot cron pipelines. `bump`
/// counts each daemon-issued self-heal re-fire; any agent/human-created job
/// for the same `(chain_id, step)` resets the budget to zero (ground-truth
/// progress). At `attempts >= MAX_AUTO_RESUMES` the daemon stops re-firing
/// and alerts workspace admins instead.
fn migrate_v32(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chain_resume_state (
            chain_id TEXT NOT NULL,
            step INTEGER NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (chain_id, step)
        );",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![32, "Chain resume ledger: chain_resume_state table (stall auto-resume)"],
    )?;
    Ok(())
}

/// v33: 用户侧票据仓库（借用机制待建清单 #5）。会话真源在用户侧——
/// App/桌面/CLI 把借道轮返回的 SessionTicket 存在这里，下次借用取回提交。
/// 键 = (agent_name, label)：同一分身可有多条并行对话（label 是用户侧的
/// 对话名，如 "default" / "工作"）。ticket_json 全量存 JSON（wire 同形状，
/// 与 ACP 直通零转换）。
fn migrate_v33(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS borrow_tickets (
            agent_name TEXT NOT NULL,
            label TEXT NOT NULL,
            ticket_json TEXT NOT NULL,
            message_count INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (agent_name, label)
        );",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (?1, datetime('now'), ?2)",
        rusqlite::params![33, "User-side ticket store: borrow_tickets table (borrowing session persistence)"],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_migrations;

    #[test]
    fn test_migration_creates_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"agents".to_string()));
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"kv_store".to_string()));
        assert!(tables.contains(&"kv_history".to_string()));

        // v26 dropped these dead tables — they have no readers/writers anywhere
        // in the codebase. Asserting their absence also guards against accidental
        // re-introduction.
        for dead in &[
            "memories",
            "entities",
            "relations",
            "canonical_sessions",
            "paired_devices",
        ] {
            assert!(
                !tables.contains(&dead.to_string()),
                "dead table {dead} should have been dropped by migrate_v26"
            );
        }

        // v27: per-user isolation column on the four content-bearing mem_tree tables.
        for table in &[
            "mem_tree_chunks",
            "mem_tree_trees",
            "mem_tree_summaries",
            "mem_tree_entity_index",
        ] {
            let cols: Vec<String> = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            assert!(
                cols.contains(&"user_id".to_string()),
                "v27 should add a user_id column to {table}"
            );
        }
    }

    #[test]
    fn test_migration_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // Should not error
    }
}
