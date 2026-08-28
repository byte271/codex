use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::KernelError;
use crate::ids::{ContentHash, SnapshotId};

/// Immutable, hash-addressed context store with copy-on-write snapshots.
pub struct ContentStore {
    root: PathBuf,
    refs: BTreeMap<ContentHash, u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub parent: Option<SnapshotId>,
    pub chunks: Vec<ContentHash>,
    pub byte_len: u64,
}

impl ContentStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, KernelError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("chunks"))?;
        fs::create_dir_all(root.join("snapshots"))?;
        let refs = load_refs(&root)?;
        Ok(Self { root, refs })
    }

    pub fn put_chunk(&mut self, bytes: &[u8]) -> Result<ContentHash, KernelError> {
        let hash = ContentHash::of_bytes(bytes);
        let path = self.chunk_path(hash);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, bytes)?;
            fs::rename(tmp, &path)?;
        }
        *self.refs.entry(hash).or_insert(0) += 1;
        self.persist_refs()?;
        Ok(hash)
    }

    pub fn get_chunk(&self, hash: ContentHash) -> Result<Vec<u8>, KernelError> {
        let path = self.chunk_path(hash);
        fs::read(&path).map_err(|err| KernelError::Context(format!("missing chunk {hash}: {err}")))
    }

    pub fn put_blob(
        &mut self,
        bytes: &[u8],
        chunk_size: usize,
    ) -> Result<Vec<ContentHash>, KernelError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let chunk_size = chunk_size.max(1);
        let mut hashes = Vec::new();
        for part in bytes.chunks(chunk_size) {
            hashes.push(self.put_chunk(part)?);
        }
        Ok(hashes)
    }

    pub fn reconstruct(&self, chunks: &[ContentHash]) -> Result<Vec<u8>, KernelError> {
        let mut out = Vec::new();
        for hash in chunks {
            let stored = self.get_chunk(*hash)?;
            if ContentHash::of_bytes(&stored) != *hash {
                return Err(KernelError::Context(format!(
                    "chunk {hash} failed corruption detection"
                )));
            }
            out.extend_from_slice(&stored);
        }
        Ok(out)
    }

    pub fn snapshot(
        &mut self,
        parent: Option<&Snapshot>,
        blob: &[u8],
        chunk_size: usize,
    ) -> Result<Snapshot, KernelError> {
        let chunks = self.put_blob(blob, chunk_size)?;
        let snap = Snapshot {
            id: SnapshotId::new(),
            parent: parent.map(|p| p.id),
            chunks,
            byte_len: blob.len() as u64,
        };
        let path = self
            .root
            .join("snapshots")
            .join(format!("{}.json", snap.id));
        fs::write(path, serde_json::to_vec_pretty(&SnapshotWire::from(&snap))?)?;
        Ok(snap)
    }

    /// Copy-on-write child: shares unchanged prefix chunks with the parent.
    pub fn snapshot_delta(
        &mut self,
        parent: &Snapshot,
        blob: &[u8],
        chunk_size: usize,
    ) -> Result<Snapshot, KernelError> {
        let chunk_size = chunk_size.max(1);
        let mut chunks = Vec::new();
        let parent_chunks = &parent.chunks;
        for (i, part) in blob.chunks(chunk_size).enumerate() {
            let hash = ContentHash::of_bytes(part);
            if parent_chunks.get(i) == Some(&hash) {
                *self.refs.entry(hash).or_insert(0) += 1;
                chunks.push(hash);
            } else {
                chunks.push(self.put_chunk(part)?);
            }
        }
        self.persist_refs()?;
        let snap = Snapshot {
            id: SnapshotId::new(),
            parent: Some(parent.id),
            chunks,
            byte_len: blob.len() as u64,
        };
        let path = self
            .root
            .join("snapshots")
            .join(format!("{}.json", snap.id));
        fs::write(path, serde_json::to_vec_pretty(&SnapshotWire::from(&snap))?)?;
        Ok(snap)
    }

    pub fn unique_bytes(&self) -> Result<u64, KernelError> {
        let mut total = 0u64;
        let chunks = self.root.join("chunks");
        if chunks.exists() {
            for entry in walk_files(&chunks)? {
                total += fs::metadata(entry)?.len();
            }
        }
        Ok(total)
    }

    pub fn referenced_bytes(&self, snapshots: &[Snapshot]) -> u64 {
        snapshots.iter().map(|s| s.byte_len).sum()
    }

    pub fn gc(&mut self) -> Result<u64, KernelError> {
        let mut removed = 0u64;
        let live: Vec<ContentHash> = self
            .refs
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(h, _)| *h)
            .collect();
        let live_set: std::collections::BTreeSet<_> = live.into_iter().collect();
        let chunks = self.root.join("chunks");
        if chunks.exists() {
            for path in walk_files(&chunks)? {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if let Ok(hash) = ContentHash::from_hex(name) {
                        if !live_set.contains(&hash) {
                            let len = fs::metadata(&path)?.len();
                            fs::remove_file(&path)?;
                            removed += len;
                        }
                    }
                }
            }
        }
        self.refs.retain(|_, count| *count > 0);
        self.persist_refs()?;
        Ok(removed)
    }

    fn chunk_path(&self, hash: ContentHash) -> PathBuf {
        let hex = hash.to_hex();
        self.root.join("chunks").join(&hex[..2]).join(hex)
    }

    fn persist_refs(&self) -> Result<(), KernelError> {
        let map: BTreeMap<String, u64> = self.refs.iter().map(|(k, v)| (k.to_hex(), *v)).collect();
        fs::write(
            self.root.join("refs.json"),
            serde_json::to_vec_pretty(&map)?,
        )?;
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotWire {
    id: String,
    parent: Option<String>,
    chunks: Vec<String>,
    byte_len: u64,
}

impl From<&Snapshot> for SnapshotWire {
    fn from(value: &Snapshot) -> Self {
        Self {
            id: value.id.to_string(),
            parent: value.parent.map(|p| p.to_string()),
            chunks: value.chunks.iter().map(|c| c.to_hex()).collect(),
            byte_len: value.byte_len,
        }
    }
}

fn load_refs(root: &Path) -> Result<BTreeMap<ContentHash, u64>, KernelError> {
    let path = root.join("refs.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw: BTreeMap<String, u64> = serde_json::from_slice(&fs::read(path)?)?;
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        out.insert(ContentHash::from_hex(&k)?, v);
    }
    Ok(out)
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>, KernelError> {
    let mut out = Vec::new();
    fn rec(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), KernelError> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                rec(&path, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }
    rec(root, &mut out)?;
    Ok(out)
}

/// Naive Codex-like child context duplication: each child stores a full copy.
pub fn naive_child_storage_bytes(
    parent_bytes: u64,
    child_delta_bytes: u64,
    n_children: u64,
) -> u64 {
    parent_bytes + n_children * (parent_bytes + child_delta_bytes)
}

/// Shared snapshot + per-child delta.
pub fn shared_child_storage_bytes(
    parent_bytes: u64,
    child_delta_bytes: u64,
    n_children: u64,
) -> u64 {
    parent_bytes + n_children * child_delta_bytes
}
