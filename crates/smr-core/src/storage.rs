use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json;

use crate::audit::RequestAudit;
use crate::events::EventRecord;

pub struct AuditStore {
    conn: Mutex<Connection>,
}

impl AuditStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("smr.db");
        let conn = Connection::open(&db_path).context("open sqlite db")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                kind TEXT NOT NULL,
                message TEXT NOT NULL,
                rule_id TEXT
            );
            CREATE TABLE IF NOT EXISTS audits (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                session_id TEXT NOT NULL,
                protocol TEXT NOT NULL,
                fallback_group TEXT NOT NULL,
                fallback_chain TEXT NOT NULL,
                final_model TEXT,
                dlp_replacements INTEGER NOT NULL,
                safety_blocks INTEGER NOT NULL,
                safety_observations INTEGER NOT NULL,
                success INTEGER NOT NULL,
                message TEXT NOT NULL
            );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("data")
    }

    pub fn insert_event(&self, record: &EventRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events (timestamp, kind, message, rule_id) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.timestamp.to_rfc3339(),
                format!("{:?}", record.kind),
                record.message,
                record.rule_id,
            ],
        )?;
        Ok(())
    }

    pub fn insert_audit(&self, audit: &RequestAudit) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO audits (id, timestamp, session_id, protocol, fallback_group, fallback_chain, final_model, dlp_replacements, safety_blocks, safety_observations, success, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                audit.id,
                audit.timestamp.to_rfc3339(),
                audit.session_id,
                audit.protocol,
                audit.fallback_group,
                serde_json::to_string(&audit.fallback_chain)?,
                audit.final_model,
                audit.dlp_replacements,
                audit.safety_blocks,
                audit.safety_observations,
                audit.success as i32,
                audit.message,
            ],
        )?;
        Ok(())
    }

    pub fn list_audits(&self, limit: usize) -> Result<Vec<RequestAudit>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, session_id, protocol, fallback_group, fallback_chain, final_model, dlp_replacements, safety_blocks, safety_observations, success, message
             FROM audits ORDER BY timestamp DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            let ts: String = row.get(1)?;
            let chain: String = row.get(5)?;
            Ok(RequestAudit {
                id: row.get(0)?,
                timestamp: DateTime::parse_from_rfc3339(&ts)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                session_id: row.get(2)?,
                protocol: row.get(3)?,
                fallback_group: row.get(4)?,
                fallback_chain: serde_json::from_str(&chain).unwrap_or_default(),
                final_model: row.get(6)?,
                dlp_replacements: row.get::<_, i64>(7)? as u32,
                safety_blocks: row.get::<_, i64>(8)? as u32,
                safety_observations: row.get::<_, i64>(9)? as u32,
                success: row.get::<_, i64>(10)? != 0,
                message: row.get(11)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
