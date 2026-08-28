use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{KernelError, LogCorruptKind};
use crate::event::{EventRecord, LOG_MAGIC, SCHEMA_VERSION};

/// File header: magic (4) + schema (2) + reserved (2).
pub const LOG_HEADER_LEN: u64 = 8;

/// Largest accepted JSON payload. A corrupted length prefix must not allocate.
pub const MAX_RECORD_BYTES: u32 = 16 * 1024 * 1024;

/// Crash-safe append-only event log.
///
/// On-disk layout:
///   magic[4] = b"CAK1"
///   schema[2] = u16 LE
///   reserved[2] = 0
///   records...
///
/// Each record:
///   len: u32 LE
///   crc32: u32 LE of payload
///   payload: JSON bytes of EventRecord
///
/// Recovery:
/// - Incomplete tail (short length, CRC, or payload at EOF) is a torn append:
///   truncate to the last complete record and continue.
/// - A *complete* record with CRC / checksum / JSON failure is fatal, including
///   when it is the last bytes of the file. Bitrot is not treated as a crash.
/// - Claimed length 0 or greater than [`MAX_RECORD_BYTES`] is fatal before
///   allocation, even if the file is shorter than that length.
pub struct EventLog {
    path: PathBuf,
    file: File,
    records: Vec<EventRecord>,
    torn_tail_at: Option<u64>,
}

impl EventLog {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, KernelError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)?;
        file.write_all(LOG_MAGIC)?;
        file.write_all(&SCHEMA_VERSION.to_le_bytes())?;
        file.write_all(&0u16.to_le_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(Self {
            path,
            file,
            records: Vec::new(),
            torn_tail_at: None,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, KernelError> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let (records, torn_tail_at) = read_records(&mut file, &path)?;
        Ok(Self {
            path,
            file,
            records,
            torn_tail_at,
        })
    }

    pub fn open_or_create(path: impl AsRef<Path>) -> Result<Self, KernelError> {
        let path = path.as_ref();
        if path.exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn records(&self) -> &[EventRecord] {
        &self.records
    }

    /// Byte offset of a recovered incomplete tail, if open truncated one.
    pub fn torn_tail_at(&self) -> Option<u64> {
        self.torn_tail_at
    }

    pub fn last(&self) -> Option<&EventRecord> {
        self.records.last()
    }

    pub fn next_seq(&self) -> u64 {
        self.records.last().map(|r| r.seq + 1).unwrap_or(1)
    }

    pub fn last_event_id(&self) -> Option<crate::ids::EventId> {
        self.records.last().map(|r| r.event_id)
    }

    pub fn append(&mut self, record: EventRecord) -> Result<EventRecord, KernelError> {
        let record = record.with_checksum()?;
        record.verify_checksum()?;
        let payload = serde_json::to_vec(&record)?;
        let len = u32::try_from(payload.len()).map_err(|_| {
            KernelError::log_corrupt(
                0,
                LogCorruptKind::RecordTooLarge,
                "record larger than u32::MAX",
            )
        })?;
        if len == 0 || len > MAX_RECORD_BYTES {
            return Err(KernelError::log_corrupt(
                0,
                LogCorruptKind::RecordTooLarge,
                format!("refusing to append {len} byte record"),
            ));
        }
        let crc = crc32fast::hash(&payload);
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.file.flush()?;
        self.file.sync_data()?;
        self.records.push(record.clone());
        Ok(record)
    }

    /// Incomplete append: claimed length exceeds the bytes that follow.
    pub fn inject_torn_write(&mut self, partial: &[u8]) -> Result<(), KernelError> {
        let crc = crc32fast::hash(partial);
        let claimed_len = (partial.len() as u32).saturating_add(64);
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&claimed_len.to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(partial)?;
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Append raw bytes after the current end (tests: truncated headers, garbage).
    pub fn inject_raw_bytes(&mut self, bytes: &[u8]) -> Result<(), KernelError> {
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Write a framed record with an optional CRC override.
    pub fn inject_framed(
        &mut self,
        payload: &[u8],
        crc: Option<u32>,
        claimed_len: Option<u32>,
    ) -> Result<(), KernelError> {
        let len = claimed_len.unwrap_or(payload.len() as u32);
        let crc = crc.unwrap_or_else(|| crc32fast::hash(payload));
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(payload)?;
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    pub fn file_len(&self) -> Result<u64, KernelError> {
        Ok(self.file.metadata()?.len())
    }
}

enum Frame {
    Complete { pos: u64, payload: Vec<u8> },
    Torn { pos: u64 },
}

fn read_records(
    file: &mut File,
    path: &Path,
) -> Result<(Vec<EventRecord>, Option<u64>), KernelError> {
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header)?;
    if &header[0..4] != LOG_MAGIC {
        return Err(KernelError::LogIntegrity(format!(
            "bad magic in {}",
            path.display()
        )));
    }
    let schema = u16::from_le_bytes([header[4], header[5]]);
    if schema != SCHEMA_VERSION {
        return Err(KernelError::UnsupportedSchema {
            found: schema,
            expected: SCHEMA_VERSION,
        });
    }
    let mut records = Vec::new();
    let mut torn_tail_at = None;
    loop {
        match read_frame(file)? {
            None => break,
            Some(Frame::Torn { pos }) => {
                file.set_len(pos)?;
                file.seek(SeekFrom::End(0))?;
                torn_tail_at = Some(pos);
                break;
            }
            Some(Frame::Complete { pos, payload }) => {
                let record: EventRecord = serde_json::from_slice(&payload).map_err(|err| {
                    KernelError::log_corrupt(pos, LogCorruptKind::InvalidJson, err.to_string())
                })?;
                if let Err(err) = record.verify_checksum() {
                    return Err(KernelError::log_corrupt(
                        pos,
                        LogCorruptKind::ChecksumMismatch,
                        err.to_string(),
                    ));
                }
                records.push(record);
            }
        }
    }
    Ok((records, torn_tail_at))
}

fn read_frame(file: &mut File) -> Result<Option<Frame>, KernelError> {
    let pos = file.stream_position()?;
    let file_len = file.metadata()?.len();
    let remaining = file_len.saturating_sub(pos);
    if remaining == 0 {
        return Ok(None);
    }
    if remaining < 4 {
        return Ok(Some(Frame::Torn { pos }));
    }
    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len == 0 || len > MAX_RECORD_BYTES {
        return Err(KernelError::log_corrupt(
            pos,
            if len == 0 {
                LogCorruptKind::InvalidLength
            } else {
                LogCorruptKind::RecordTooLarge
            },
            format!("claimed length {len}, max {MAX_RECORD_BYTES}"),
        ));
    }
    let remaining_after_len = file_len.saturating_sub(file.stream_position()?);
    if remaining_after_len < 4 {
        return Ok(Some(Frame::Torn { pos }));
    }
    let mut crc_buf = [0u8; 4];
    file.read_exact(&mut crc_buf)?;
    let expected_crc = u32::from_le_bytes(crc_buf);
    let remaining_after_crc = file_len.saturating_sub(file.stream_position()?);
    if remaining_after_crc < u64::from(len) {
        return Ok(Some(Frame::Torn { pos }));
    }
    let mut payload = vec![0u8; len as usize];
    file.read_exact(&mut payload)?;
    let actual_crc = crc32fast::hash(&payload);
    if actual_crc != expected_crc {
        return Err(KernelError::log_corrupt(
            pos,
            LogCorruptKind::CrcMismatch,
            format!("expected {expected_crc:#x}, got {actual_crc:#x}"),
        ));
    }
    Ok(Some(Frame::Complete { pos, payload }))
}

pub fn replay_file(path: impl AsRef<Path>) -> Result<crate::state::KernelState, KernelError> {
    let log = EventLog::open(path)?;
    crate::reducer::reduce_all(log.records())
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;
