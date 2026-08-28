use super::{EventLog, MAX_RECORD_BYTES};
use crate::error::{KernelError, LogCorruptKind};
use crate::event::Event;
use crate::ids::IdempotencyKey;
use crate::kernel::Kernel;
use pretty_assertions::assert_eq;

fn open_err(path: &std::path::Path) -> KernelError {
    match EventLog::open(path) {
        Err(err) => err,
        Ok(_) => panic!("expected log open to fail"),
    }
}

fn seeded_log() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.cak");
    {
        let log = EventLog::create(&path).unwrap();
        let mut k = Kernel::from_log(log).unwrap();
        k.create_execution("seed").unwrap();
    }
    (dir, path)
}

#[test]
fn torn_payload_at_eof_is_truncated() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_torn_write(b"{\"partial\":true").unwrap();
    }
    let log = EventLog::open(&path).unwrap();
    assert_eq!(log.records().len(), 1);
    assert!(log.torn_tail_at().is_some());
    Kernel::from_log(log).unwrap();
}

#[test]
fn truncated_length_prefix_is_torn() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_raw_bytes(&[1, 2, 3]).unwrap();
    }
    let log = EventLog::open(&path).unwrap();
    assert_eq!(log.records().len(), 1);
    assert!(log.torn_tail_at().is_some());
}

#[test]
fn truncated_crc_is_torn() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_raw_bytes(&[10, 0, 0, 0, 1, 2]).unwrap();
    }
    let log = EventLog::open(&path).unwrap();
    assert_eq!(log.records().len(), 1);
    assert!(log.torn_tail_at().is_some());
}

#[test]
fn truncated_payload_is_torn() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_framed(b"not-enough", /*crc*/ None, Some(64))
            .unwrap();
    }
    let log = EventLog::open(&path).unwrap();
    assert_eq!(log.records().len(), 1);
    assert!(log.torn_tail_at().is_some());
}

#[test]
fn complete_crc_mismatch_at_eof_is_fatal() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_framed(b"{\"x\":1}", Some(0xdead_beef), None)
            .unwrap();
    }
    let err = open_err(&path);
    match err {
        KernelError::LogCorrupt {
            kind: LogCorruptKind::CrcMismatch,
            ..
        } => {}
        other => panic!("expected crc mismatch, got {other}"),
    }
}

#[test]
fn complete_invalid_json_is_fatal() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_framed(b"not-json", None, None).unwrap();
    }
    let err = open_err(&path);
    match err {
        KernelError::LogCorrupt {
            kind: LogCorruptKind::InvalidJson,
            ..
        } => {}
        other => panic!("expected invalid json, got {other}"),
    }
}

#[test]
fn complete_checksum_mismatch_is_fatal() {
    let (_dir, path) = seeded_log();
    let mut record = {
        let log = EventLog::open(&path).unwrap();
        log.records()[0].clone()
    };
    record.checksum = "00".repeat(32);
    let payload = serde_json::to_vec(&record).unwrap();
    {
        let mut log = EventLog::open(&path).unwrap();
        // Replace the file with header + one bad-checksum framed record.
        log.inject_framed(&payload, None, None).unwrap();
    }
    // The file now has the original good record plus a second framed payload
    // whose envelope checksum does not match. Opening must fail.
    let err = open_err(&path);
    match err {
        KernelError::LogCorrupt {
            kind: LogCorruptKind::ChecksumMismatch,
            ..
        } => {}
        other => panic!("expected checksum mismatch, got {other}"),
    }
}

#[test]
fn huge_length_is_fatal_without_allocating() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        let huge = MAX_RECORD_BYTES.saturating_add(1).to_le_bytes();
        log.inject_raw_bytes(&huge).unwrap();
        log.inject_raw_bytes(&[0, 0, 0, 0]).unwrap();
    }
    let err = open_err(&path);
    match err {
        KernelError::LogCorrupt {
            kind: LogCorruptKind::RecordTooLarge,
            ..
        } => {}
        other => panic!("expected record too large, got {other}"),
    }
}

#[test]
fn zero_length_is_fatal() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_raw_bytes(&[0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    }
    let err = open_err(&path);
    match err {
        KernelError::LogCorrupt {
            kind: LogCorruptKind::InvalidLength,
            ..
        } => {}
        other => panic!("expected invalid length, got {other}"),
    }
}

#[test]
fn valid_record_followed_by_garbage_incomplete_tail_is_torn() {
    let (_dir, path) = seeded_log();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.inject_raw_bytes(&[7, 7]).unwrap();
    }
    let log = EventLog::open(&path).unwrap();
    assert_eq!(log.records().len(), 1);
    assert!(log.torn_tail_at().is_some());
}

#[test]
fn bit_flip_in_complete_payload_is_fatal() {
    let (_dir, path) = seeded_log();
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&path, bytes).unwrap();
    let err = open_err(&path);
    match err {
        KernelError::LogCorrupt {
            kind: LogCorruptKind::CrcMismatch | LogCorruptKind::InvalidJson,
            ..
        } => {}
        other => panic!("expected payload bitrot to be fatal, got {other}"),
    }
}

#[test]
fn idempotent_append_round_trip_after_clean_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.cak");
    let log = EventLog::create(&path).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("a").unwrap();
    k.append(
        Event::ExecutionCreated {
            created_at_ms: 0,
            note: "a".into(),
        },
        IdempotencyKey::new("execution_created", "a"),
    )
    .unwrap();
    assert_eq!(k.log.records().len(), 1);
}
