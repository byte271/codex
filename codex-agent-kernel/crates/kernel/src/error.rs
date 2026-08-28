use std::fmt;

use thiserror::Error;

/// Why a complete on-disk record is rejected instead of truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogCorruptKind {
    RecordTooLarge,
    CrcMismatch,
    ChecksumMismatch,
    InvalidJson,
    InvalidLength,
}

impl fmt::Display for LogCorruptKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RecordTooLarge => "record too large",
            Self::CrcMismatch => "crc mismatch",
            Self::ChecksumMismatch => "checksum mismatch",
            Self::InvalidJson => "invalid json",
            Self::InvalidLength => "invalid length",
        })
    }
}

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("invalid id for {kind}: {value} ({cause})")]
    InvalidId {
        kind: &'static str,
        value: String,
        cause: String,
    },
    #[error("invalid content hash: {0}")]
    InvalidHash(String),
    #[error("invalid transition: {0}")]
    InvalidTransition(String),
    #[error("stale lease: operation {operation} current generation {current}, commit used {used}")]
    StaleLease {
        operation: String,
        current: u64,
        used: u64,
    },
    #[error("non-monotonic sequence for execution {execution}: expected {expected}, got {got}")]
    NonMonotonicSeq {
        execution: String,
        expected: u64,
        got: u64,
    },
    #[error("duplicate idempotency key {key} bound to {existing}, not {new_event}")]
    IdempotencyConflict {
        key: String,
        existing: String,
        new_event: String,
    },
    #[error("execution not found: {0}")]
    ExecutionNotFound(String),
    #[error("event not found: {0}")]
    EventNotFound(String),
    #[error("log integrity error: {0}")]
    LogIntegrity(String),
    #[error("log corruption at byte {at_byte} ({kind}): {detail}")]
    LogCorrupt {
        at_byte: u64,
        kind: LogCorruptKind,
        detail: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite error: {0}")]
    Sqlite(String),
    #[error("process error: {0}")]
    Process(String),
    #[error("context store error: {0}")]
    Context(String),
    #[error("checksum mismatch")]
    ChecksumMismatch,
    #[error("schema version {found} is not supported (expected {expected})")]
    UnsupportedSchema { found: u16, expected: u16 },
}

impl KernelError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidTransition(msg.into())
    }

    pub fn log_corrupt(at_byte: u64, kind: LogCorruptKind, detail: impl Into<String>) -> Self {
        Self::LogCorrupt {
            at_byte,
            kind,
            detail: detail.into(),
        }
    }

    pub fn sqlite(err: rusqlite::Error) -> Self {
        Self::Sqlite(err.to_string())
    }
}
