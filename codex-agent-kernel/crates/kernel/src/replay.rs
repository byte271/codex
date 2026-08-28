use std::fs;
use std::path::{Path, PathBuf};

use crate::error::KernelError;
use crate::event::{Event, EventRecord, SCHEMA_VERSION};
use crate::ids::{CorrelationId, EventId, ExecutionId, IdempotencyKey};
use crate::kernel::Kernel;
use crate::log::EventLog;
use crate::reducer::reduce_all;
use crate::state::{business_hash, prefix_hashes, state_hash};

pub struct ReplayReport {
    pub execution_id: ExecutionId,
    pub events: usize,
    pub state_hash: String,
    pub business_hash: String,
    pub prefix_hashes: Vec<(String, String)>,
    pub tree: String,
    pub trace: String,
}

pub fn replay_until(
    log_path: impl AsRef<Path>,
    until: Option<EventId>,
) -> Result<ReplayReport, KernelError> {
    let log = EventLog::open(log_path)?;
    let mut records: Vec<EventRecord> = Vec::new();
    let mut found = until.is_none();
    for record in log.records() {
        records.push(record.clone());
        if let Some(until) = until {
            if record.event_id == until {
                found = true;
                break;
            }
        }
    }
    if let Some(until) = until {
        if !found {
            return Err(KernelError::EventNotFound(until.to_string()));
        }
    }
    let state = reduce_all(&records)?;
    let execution_id = records
        .first()
        .map(|r| r.execution_id)
        .ok_or_else(|| KernelError::invalid("empty log"))?;
    let exec = state.require_execution(execution_id)?;
    let hashes = prefix_hashes(&records)?;
    Ok(ReplayReport {
        execution_id,
        events: records.len(),
        state_hash: state_hash(exec)?.to_hex(),
        business_hash: business_hash(exec)?.to_hex(),
        prefix_hashes: hashes
            .into_iter()
            .map(|(id, sh, _bh)| (id.to_string(), sh.to_hex()))
            .collect(),
        tree: crate::viewer::render_tree(exec),
        trace: crate::viewer::render_trace(&records, until)?,
    })
}

pub fn fork_from(
    source_log: impl AsRef<Path>,
    dest_dir: impl AsRef<Path>,
    from_event_id: EventId,
) -> Result<(ExecutionId, String, String), KernelError> {
    let source = EventLog::open(source_log)?;
    let mut prefix = Vec::new();
    let mut from_seq = None;
    for record in source.records() {
        prefix.push(record.clone());
        if record.event_id == from_event_id {
            from_seq = Some(record.seq);
            break;
        }
    }
    let from_seq = from_seq.ok_or_else(|| KernelError::EventNotFound(from_event_id.to_string()))?;
    let source_state = reduce_all(&prefix)?;
    let source_id = prefix[0].execution_id;
    let source_exec = source_state.require_execution(source_id)?;
    let source_business = business_hash(source_exec)?.to_hex();

    let dest_dir = dest_dir.as_ref();
    fs::create_dir_all(dest_dir)?;
    let dest_path = dest_dir.join("events.cak");
    let mut dest = EventLog::create(&dest_path)?;
    let new_id = ExecutionId::new();
    let mut rewritten = Vec::new();
    for record in prefix {
        let mut rec = record;
        rec.execution_id = new_id;
        rec.correlation_id = Some(CorrelationId::from_uuid(new_id.as_uuid()));
        rec.checksum.clear();
        rewritten.push(rec.with_checksum()?);
    }
    for rec in rewritten {
        dest.append(rec)?;
    }
    let fork_event = EventRecord {
        schema_version: SCHEMA_VERSION,
        execution_id: new_id,
        event_id: EventId::new(),
        seq: dest.next_seq(),
        idempotency_key: IdempotencyKey::new("fork", new_id),
        causation_id: Some(from_event_id),
        correlation_id: Some(CorrelationId::from_uuid(new_id.as_uuid())),
        parent_event_id: dest.last_event_id(),
        occurred_at_ms: 0,
        checksum: String::new(),
        payload: Event::ExecutionForked {
            source_execution_id: source_id,
            from_event_id,
            from_seq,
            source_business_hash: source_business.clone(),
        },
    }
    .with_checksum()?;
    dest.append(fork_event)?;

    let forked = Kernel::from_log(dest)?;
    let forked_exec = forked.execution()?;
    let forked_business = business_hash(forked_exec)?.to_hex();
    if forked_business != source_business {
        // Fork event is extra metadata; business view includes ancestry. Compare
        // the prefix-only rewrite against the source instead.
    }
    let prefix_only = reduce_all(&forked.log.records()[..forked.log.records().len() - 1])?;
    let prefix_exec = prefix_only.require_execution(new_id)?;
    let prefix_business = business_hash(prefix_exec)?.to_hex();
    if prefix_business != source_business {
        return Err(KernelError::invalid(format!(
            "fork divergence: source {source_business} vs fork prefix {prefix_business}"
        )));
    }
    fs::write(
        dest_dir.join("ANCESTRY.txt"),
        format!("source={source_id}\nfrom_event={from_event_id}\nfrom_seq={from_seq}\n"),
    )?;
    Ok((new_id, source_business, prefix_business))
}

pub fn copy_kernel_dir(
    src: impl AsRef<Path>,
    dest: impl AsRef<Path>,
) -> Result<PathBuf, KernelError> {
    let dest = dest.as_ref().to_path_buf();
    fs::create_dir_all(&dest)?;
    let from = src.as_ref().join("events.cak");
    if from.exists() {
        fs::copy(&from, dest.join("events.cak"))?;
    }
    Ok(dest)
}
