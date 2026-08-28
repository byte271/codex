use rusqlite::{params, Connection};
use std::path::Path;

use crate::error::KernelError;
use crate::event::EventRecord;
use crate::ids::ExecutionId;
use crate::state::{state_hash, ExecutionState, KernelState};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS executions (
    execution_id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    note TEXT NOT NULL,
    state_hash TEXT NOT NULL,
    business_hash TEXT NOT NULL,
    last_seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    execution_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    checksum TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (execution_id, seq)
);
CREATE TABLE IF NOT EXISTS operations (
    execution_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    status TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    lease_generation INTEGER,
    committed_attempt TEXT,
    PRIMARY KEY (execution_id, operation_id)
);
"#;

/// Rebuildable SQLite projection. Never authoritative.
pub struct Projection {
    conn: Connection,
}

impl Projection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, KernelError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path.as_ref()).map_err(KernelError::sqlite)?;
        conn.execute_batch(SCHEMA).map_err(KernelError::sqlite)?;
        Ok(Self { conn })
    }

    pub fn rebuild(
        &mut self,
        state: &KernelState,
        records: &[EventRecord],
    ) -> Result<(), KernelError> {
        self.conn
            .execute_batch("DELETE FROM executions; DELETE FROM events; DELETE FROM operations;")
            .map_err(KernelError::sqlite)?;
        self.project(state, records)
    }

    pub fn project(
        &mut self,
        state: &KernelState,
        records: &[EventRecord],
    ) -> Result<(), KernelError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(KernelError::sqlite)?;
        for record in records {
            tx.execute(
                "INSERT OR REPLACE INTO events
                    (execution_id, seq, event_id, idempotency_key, checksum, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.execution_id.to_string(),
                    record.seq as i64,
                    record.event_id.to_string(),
                    record.idempotency_key.0,
                    record.checksum,
                    serde_json::to_string(&record.payload)?,
                ],
            )
            .map_err(KernelError::sqlite)?;
        }
        for exec in state.executions.values() {
            insert_execution(&tx, exec)?;
        }
        tx.commit().map_err(KernelError::sqlite)?;
        Ok(())
    }

    pub fn load_execution_json(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Option<String>, KernelError> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload_json FROM executions WHERE execution_id = ?1")
            .map_err(KernelError::sqlite)?;
        let mut rows = stmt
            .query(params![execution_id.to_string()])
            .map_err(KernelError::sqlite)?;
        match rows.next().map_err(KernelError::sqlite)? {
            Some(row) => Ok(Some(row.get(0).map_err(KernelError::sqlite)?)),
            None => Ok(None),
        }
    }

    pub fn load_execution(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Option<ExecutionState>, KernelError> {
        match self.load_execution_json(execution_id)? {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub fn corrupt_execution_row(&self, execution_id: ExecutionId) -> Result<(), KernelError> {
        self.conn
            .execute(
                "UPDATE executions SET payload_json = '{not-json' WHERE execution_id = ?1",
                params![execution_id.to_string()],
            )
            .map_err(KernelError::sqlite)?;
        Ok(())
    }
}

fn insert_execution(
    tx: &rusqlite::Transaction<'_>,
    exec: &ExecutionState,
) -> Result<(), KernelError> {
    tx.execute(
        "INSERT OR REPLACE INTO executions
            (execution_id, created_at_ms, note, state_hash, business_hash, last_seq, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            exec.id.to_string(),
            exec.created_at_ms,
            exec.note,
            crate::state::state_hash(exec)?.to_hex(),
            crate::state::business_hash(exec)?.to_hex(),
            exec.checkpoints.last().map(|c| c.seq as i64).unwrap_or(0),
            serde_json::to_string(exec)?,
        ],
    )
    .map_err(KernelError::sqlite)?;
    for op in exec.operations.values() {
        tx.execute(
            "INSERT OR REPLACE INTO operations
                (execution_id, operation_id, status, agent_id, lease_generation, committed_attempt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                exec.id.to_string(),
                op.id.to_string(),
                format!("{:?}", op.status),
                op.agent_id.to_string(),
                op.lease.as_ref().map(|l| l.generation as i64),
                op.committed_attempt.map(|id| id.to_string()),
            ],
        )
        .map_err(KernelError::sqlite)?;
    }
    let _ = state_hash(exec)?;
    Ok(())
}
