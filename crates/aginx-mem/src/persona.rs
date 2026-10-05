//! ④-5 人格真源导出（2026-10-05）：carrier.db kv 人格域 → workspace
//! `knowledge/*.md` + MEMORY.md 真填；敏感键经 `aginx-secret set` 入
//! secretd（scope `persona.*`）；`--prune` 备份并 DROP 引擎孤儿空表。
//!
//! 用户三裁决：A 分层（明文进 knowledge/、敏感进 secret）；B 刀内删
//! （备份+drop）；C 文件树=真源，db kv 域封存不删（历史账；生态网关
//! 后续写入算流水账——差异记 docs/REQ-aginx-kv-ledger.md）。
//!
//! 对 db 走裸 rusqlite、不经 substrate open——不在在役库上跑迁移，
//! 避免与本仓 migration 清单分叉。secret 投递走 `aginx-secret set`
//! 子进程：put 是 admin 面（/etc/aginx/secret.policy 的 admin 伪 scope
//! 只认 /usr/bin/aginx-secret），库直连必被拒——spawn 正路即守约。

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::knowledge::{knowledge_add_core, knowledge_index};

/// 与 knowledge.rs reject_credential_like 同表：内容形状像凭证即敏感。
const CREDENTIAL_PATTERNS: &[&str] = &[
    "app_secret",
    "app_id",
    "api_key",
    "apikey",
    "secret_key",
    "access_token",
    "private_key",
];

/// 键名含这些词即敏感（电话簿/账号/凭证类人格数据）。
const SENSITIVE_KEY_MARKS: &[&str] = &[
    "phone", "account", "password", "passwd", "token", "secret", "credential", "contact",
];

/// 刀④-4 死码对应的引擎孤儿空表（0 行、无在役读者）。mem_tree 三���空表
/// 是活 memory 线的 schema、cron_jobs 邻接生态网关——都不在列。
pub const ORPHAN_TABLES: &[&str] = &[
    "automation_rules",
    "borrow_tickets",
    "chain_resume_state",
    "channel_followers",
    "events",
    "flow_runs",
    "invites",
    "notify_routes",
    "pending_notifications",
    "task_queue",
];

fn is_sensitive(key: &str, serialized: &str) -> bool {
    let k = key.to_lowercase();
    if SENSITIVE_KEY_MARKS.iter().any(|m| k.contains(m)) {
        return true;
    }
    let s = serialized.to_lowercase();
    CREDENTIAL_PATTERNS.iter().any(|p| s.contains(p))
}

fn render_body(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => format!("```json\n{}\n```", serde_json::to_string_pretty(other).unwrap_or_default()),
    }
}

/// `aginx-secret set persona.<agent>.<key>`，值走 stdin（argv 不落地）。
fn put_secret(scope: &str, value: &str) -> Result<()> {
    let mut child = Command::new("aginx-secret")
        .arg("set")
        .arg(scope)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("spawn aginx-secret（设备上在 /usr/bin；host 测试用 --no-secret）")?;
    child
        .stdin
        .take()
        .context("aginx-secret stdin 不可用")?
        .write_all(value.as_bytes())?;
    let out = child.wait().context("wait aginx-secret")?;
    if !out.success() {
        bail!("aginx-secret set {scope} 失败（exit {:?}）", out.code());
    }
    Ok(())
}

/// 备份（VACUUM INTO 一致快照，含 WAL 内容）+ DROP 孤儿表。
fn prune_orphans(conn: &Connection, db: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_dir = db
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".pre-knife45");
    std::fs::create_dir_all(&backup_dir)
        .with_context(|| format!("mkdir {}", backup_dir.display()))?;
    let backup = backup_dir.join(format!("carrier-{stamp}.db"));
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
        .with_context(|| format!("VACUUM INTO {}", backup.display()))?;
    // 备份可开才许动真库
    let vconn = Connection::open(&backup)?;
    let tables: i64 = vconn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table'",
        [],
        |r| r.get(0),
    )?;
    if tables == 0 {
        bail!("备份 {} 空库，中止 prune", backup.display());
    }
    for t in ORPHAN_TABLES {
        conn.execute(&format!("DROP TABLE IF EXISTS {t}"), [])
            .with_context(|| format!("drop {t}"))?;
    }
    Ok(backup)
}

/// 入口：`aginx-mem persona --db <carrier.db> --workspace <home>`。
/// secret=true 投 secretd；false 时敏感条目只跳过（host 测试位）。
/// prune=true 先导出后瘦身（备份→验→drop）。
pub async fn run(db: &Path, workspace: &Path, secret: bool, prune: bool) -> Result<()> {
    let conn = Connection::open(db)
        .with_context(|| format!("open {}", db.display()))?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .context("busy_timeout")?;

    let mut stmt = conn
        .prepare("SELECT agent_id, owner_id, user_id, key, value FROM kv_store ORDER BY agent_id, key")
        .context("读 kv_store（库形状不对？）")?;
    let rows: Vec<(String, String, String, String, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<std::result::Result<_, _>>()
        .context("遍历 kv_store")?;
    drop(stmt);

    // kv 主键是 (agent,owner,user,key) 四元——同 (agent,key) 多身份三元
    // 常见（system 的 loop_state 有 13 条）。按 (agent,key) 分组合并渲染，
    // 逐条直写会互相覆写同名文件（首跑实锤）。
    use std::collections::BTreeMap;
    let mut grouped: BTreeMap<(String, String), Vec<(String, String, Value)>> = BTreeMap::new();
    for (agent, owner, user, key, blob) in rows {
        let value: Value = serde_json::from_slice(&blob)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&blob).into_owned()));
        grouped.entry((agent, key)).or_default().push((owner, user, value));
    }

    let mut n_knowledge = 0usize;
    let mut n_secret = 0usize;
    let mut secret_scopes: Vec<String> = Vec::new();
    for ((agent, key), mut triples) in grouped {
        // 同值去重；多值按身份三元分节，最后一条作敏感判据
        let serialized = serde_json::to_string(&triples.last().unwrap().2).unwrap_or_default();
        let title = format!("{agent}·{key}");
        let (body, sensitive) = if triples.len() == 1 {
            (render_body(&triples[0].2), is_sensitive(&key, &serialized))
        } else {
            triples.dedup_by(|a, b| a.2 == b.2);
            let any_sensitive = triples
                .iter()
                .any(|(_, _, v)| is_sensitive(&key, &serde_json::to_string(v).unwrap_or_default()));
            let sections = triples
                .iter()
                .map(|(owner, user, v)| {
                    format!("## {owner} / {user}\n\n{}", render_body(v))
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            (sections, any_sensitive)
        };
        if sensitive {
            let scope = format!("persona.{agent}.{key}");
            if secret {
                put_secret(&scope, &body)?;
                println!("secret     {title}（{} 三元）→ {scope}", triples.len());
                n_secret += 1;
            } else {
                println!("skip       {title}（敏感，--no-secret 不导出）");
            }
            secret_scopes.push(scope);
            continue;
        }
        let filename = knowledge_add_core(workspace, &title, &body, "persona-export")
            .await
            .map_err(|e| anyhow::anyhow!("写 {title}: {e}"))?;
        println!(
            "knowledge  {filename}.md ← {title}（{} 三元，{}B）",
            triples.len(),
            body.len()
        );
        n_knowledge += 1;
    }

    // 指针文件：知识树里声明敏感条目住在 secret 侧（含 no-secret 未投递
    // 场景——scope 名册照样在，人看完再决定投不投）。
    if !secret_scopes.is_empty() {
        let body = format!(
            "以下人格条目含敏感数据（电话簿/账号/凭证类），明文不入知识库，\n已存 secret 侧（--no-secret 下未投递）。读取：`aginx-secret get <scope>`。\n\n{}\n",
            secret_scopes
                .iter()
                .map(|s| format!("- `{s}`"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        knowledge_add_core(workspace, "敏感条目清单", &body, "persona-export")
            .await
            .map_err(|e| anyhow::anyhow!("写敏感清单: {e}"))?;
        println!("knowledge  敏感条目清单.md（{} 条 scope 名册）", secret_scopes.len());
    }

    knowledge_index(Some(workspace))
        .await
        .map_err(|e| anyhow::anyhow!("重建 MEMORY.md: {e}"))?;
    println!("index      MEMORY.md 已重建");

    let backup = if prune {
        let b = prune_orphans(&conn, db)?;
        println!(
            "prune      {} 张孤儿表已 DROP；备份 {}",
            ORPHAN_TABLES.len(),
            b.display()
        );
        Some(b)
    } else {
        None
    };
    println!(
        "persona-export 完成：knowledge {} 条 / secret {n_secret} 条 / 未投递 {} 条{}",
        n_knowledge + usize::from(!secret_scopes.is_empty()),
        secret_scopes.len() - n_secret,
        backup.as_ref().map(|b| format!("；prune 备份 {}", b.display())).unwrap_or_default()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fixture_db(dir: &Path) -> PathBuf {
        let db = dir.join("carrier.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE kv_store (agent_id TEXT, owner_id TEXT, user_id TEXT, key TEXT, value BLOB)",
            [],
        )
        .unwrap();
        let rows: &[(&str, &str, &str)] = &[
            ("me", "entity.misc", r#"{"偏好":["黑底绿字","简洁"]}"#),
            ("me", "profile.phone_numbers", r#"{"家":"13800000000"}"#),
            ("system", "preference.general", r#""中文为主，技术术语保留英文""#),
            ("morning-report", "晨报-20260927-要点", r#""三条要点……""#),
            ("system", "entity.accounts", r#"{"微信":"绑定号"}"#),
        ];
        for (agent, key, value) in rows {
            conn.execute(
                "INSERT INTO kv_store (agent_id, owner_id, user_id, key, value) VALUES (?1,'default','local',?2,?3)",
                params![agent, key, value.as_bytes()],
            )
            .unwrap();
        }
        // 同 (agent,key) 异 owner 三元——分组合并的形状
        conn.execute(
            "INSERT INTO kv_store (agent_id, owner_id, user_id, key, value) VALUES ('system','wechat','wx1','preference.general',?1)",
            params![r#""微信侧偏好""#.as_bytes()],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE automation_rules (id INTEGER PRIMARY KEY)",
            [],
        )
        .unwrap();
        conn.execute("CREATE TABLE task_queue (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        db
    }

    #[test]
    fn sensitive_classifier() {
        assert!(is_sensitive("profile.phone_numbers", "{}"));
        assert!(is_sensitive("entity.accounts", "{}"));
        assert!(!is_sensitive("entity.misc", "{}"));
        assert!(!is_sensitive("preference.general", "「口吻」"));
        assert!(is_sensitive("note", r#"{"api_key":"x"}"#));
    }

    #[tokio::test]
    async fn export_splits_and_prunes() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fixture_db(tmp.path());
        let ws = tmp.path().join("home");
        std::fs::create_dir_all(&ws).unwrap();

        run(&db, &ws, false, true).await.unwrap();

        let kdir = ws.join("knowledge");
        assert!(kdir.join("me_entity_misc.md").exists());
        assert!(kdir.join("system_preference_general.md").exists());
        assert!(kdir.join("morning-report_晨报-20260927-要点.md").exists());
        assert!(kdir.join("敏感条目清单.md").exists());
        // 多三元合并：分节渲染，不是覆写
        let merged = std::fs::read_to_string(kdir.join("system_preference_general.md")).unwrap();
        assert!(merged.contains("## default / local"), "缺分节：{merged}");
        assert!(merged.contains("## wechat / wx1"), "缺分节：{merged}");
        // 敏感键绝不出现在知识目录
        for f in std::fs::read_dir(&kdir).unwrap() {
            let name = f.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.contains("phone"), "敏感明文泄漏: {name}");
            assert!(!name.contains("account"), "敏感明文泄漏: {name}");
        }
        // MEMORY.md 真填
        let mem = std::fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert!(mem.contains("entity"), "MEMORY.md 未含条目：{mem}");
        // prune：孤儿表没了、备份可开
        let conn = Connection::open(&db).unwrap();
        for t in ORPHAN_TABLES {
            let n: i64 = conn
                .query_row(
                    &format!("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='{t}'"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "{t} 未删");
        }
        let backups: Vec<_> = std::fs::read_dir(tmp.path().join(".pre-knife45"))
            .unwrap()
            .collect();
        assert_eq!(backups.len(), 1);
        // kv 域封存：一行不删（裁决 C）
        let kv: i64 = conn.query_row("SELECT count(*) FROM kv_store", [], |r| r.get(0)).unwrap();
        assert_eq!(kv, 6);
    }
}
