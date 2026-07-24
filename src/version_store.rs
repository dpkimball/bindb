/// Disk-backed version history store with memory-mapped reads (issue #6).
///
/// Keeps only `(valid_from, valid_to, file_offset)` per version in RAM,
/// while the full `BinderObjectVersion` / `BinderEdgeVersion` payloads live
/// in an append-only file (`version_log.bin`).
///
/// ### Memory impact
/// Without this: O(n_total_versions × avg_version_size) in RAM.
/// With this: O(n_total_versions × ~40 bytes) in RAM for the index.
/// For a store with 5 M historical versions the index uses ~200 MB vs ~1 GB.
///
/// ### mmap read path
/// After rebuild or hydration the file is memory-mapped for reads.
/// Appends invalidate the mapping; the next read lazily re-maps, keeping
/// the file descriptor open but avoiding a seek+read syscall per lookup.
///
/// ### File format
/// Sequence of length-prefixed bincode frames:
/// ```text
/// [u64 payload_len][bincode of VersionRecord]
/// ```
///
/// `RetireObject` / `RetireEdge` records are appended when a version is
/// superseded.  This makes the file purely append-only — no frame rewriting.
///
/// ### Rebuild invariant
/// The file is a non-canonical projection.  It can be truncated and rebuilt
/// from `object_versions` / `edge_versions` (which are themselves WAL
/// projections) at any time.
use crate::edge::BinderEdgeVersion;
use crate::object::BinderObjectVersion;
use crate::types::{CommitId, EdgeId, ObjectId};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

// ── on-disk record type ───────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
enum VersionRecord {
    Object(BinderObjectVersion),
    Edge(BinderEdgeVersion),
    RetireObject { id: ObjectId, retired_at: CommitId },
    RetireEdge { id: EdgeId, retired_at: CommitId },
}

// ── in-memory index entry ─────────────────────────────────────────────────────

struct IndexEntry {
    valid_from: CommitId,
    valid_to: Option<CommitId>,
    /// Byte offset of the `Object` / `Edge` frame in the file.
    offset: u64,
}

// ── public struct ─────────────────────────────────────────────────────────────

pub struct VersionStore {
    file: File,
    write_pos: u64,
    /// Lazily refreshed read mapping. None when dirty (after an append).
    mmap: Option<Mmap>,
    object_index: HashMap<ObjectId, Vec<IndexEntry>>,
    edge_index: HashMap<EdgeId, Vec<IndexEntry>>,
}

impl VersionStore {
    pub fn open_or_create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let write_pos = file.metadata()?.len();
        let mmap = if write_pos > 0 {
            Some(unsafe { Mmap::map(&file)? })
        } else {
            None
        };
        Ok(Self {
            file,
            write_pos,
            mmap,
            object_index: HashMap::new(),
            edge_index: HashMap::new(),
        })
    }

    /// Truncate the file and rebuild it from the given version slices.
    ///
    /// Called during `open()` (after WAL replay) and `rebuild_indexes()`.
    pub fn rebuild_from_versions(
        &mut self,
        objects: &[BinderObjectVersion],
        edges: &[BinderEdgeVersion],
    ) -> io::Result<()> {
        self.mmap = None;
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.write_pos = 0;
        self.object_index.clear();
        self.edge_index.clear();

        for v in objects {
            let offset = self.append_record(&VersionRecord::Object(v.clone()))?;
            self.object_index
                .entry(v.object_id.clone())
                .or_default()
                .push(IndexEntry {
                    valid_from: v.valid_from,
                    valid_to: v.valid_to,
                    offset,
                });
        }

        for v in edges {
            let offset = self.append_record(&VersionRecord::Edge(v.clone()))?;
            self.edge_index
                .entry(v.edge_id.clone())
                .or_default()
                .push(IndexEntry {
                    valid_from: v.valid_from,
                    valid_to: v.valid_to,
                    offset,
                });
        }

        self.file.flush()?;
        self.refresh_mmap()?;
        Ok(())
    }

    /// Rebuild in-memory indexes by scanning an existing `version_log.bin`
    /// without rewriting it. Used on clean reopen when the file is already
    /// consistent with the snapshot (avoids O(n) rewrite on every `open()`).
    pub fn hydrate_index_from_file(&mut self) -> io::Result<()> {
        self.object_index.clear();
        self.edge_index.clear();
        self.file.seek(SeekFrom::Start(0))?;
        let file_len = self.file.metadata()?.len();
        let mut pos = 0u64;

        while pos + 8 <= file_len {
            self.file.seek(SeekFrom::Start(pos))?;
            let mut len_buf = [0u8; 8];
            if self.file.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u64::from_le_bytes(len_buf) as usize;
            if pos + 8 + len as u64 > file_len {
                break;
            }
            let mut buf = vec![0u8; len];
            if self.file.read_exact(&mut buf).is_err() {
                break;
            }
            let offset = pos;
            pos += 8 + len as u64;

            let record: VersionRecord = match bincode::deserialize(&buf) {
                Ok(r) => r,
                Err(_) => break,
            };
            match record {
                VersionRecord::Object(v) => {
                    self.object_index
                        .entry(v.object_id)
                        .or_default()
                        .push(IndexEntry {
                            valid_from: v.valid_from,
                            valid_to: v.valid_to,
                            offset,
                        });
                }
                VersionRecord::Edge(v) => {
                    self.edge_index
                        .entry(v.edge_id)
                        .or_default()
                        .push(IndexEntry {
                            valid_from: v.valid_from,
                            valid_to: v.valid_to,
                            offset,
                        });
                }
                VersionRecord::RetireObject { id, retired_at } => {
                    if let Some(entries) = self.object_index.get_mut(&id) {
                        if let Some(e) = entries.iter_mut().rfind(|e| e.valid_to.is_none()) {
                            e.valid_to = Some(retired_at);
                        }
                    }
                }
                VersionRecord::RetireEdge { id, retired_at } => {
                    if let Some(entries) = self.edge_index.get_mut(&id) {
                        if let Some(e) = entries.iter_mut().rfind(|e| e.valid_to.is_none()) {
                            e.valid_to = Some(retired_at);
                        }
                    }
                }
            }
        }

        self.write_pos = pos;
        self.file.seek(SeekFrom::Start(self.write_pos))?;
        self.refresh_mmap()?;
        Ok(())
    }

    /// True when the in-memory index has no entries.
    pub fn index_is_empty(&self) -> bool {
        self.object_index.is_empty() && self.edge_index.is_empty()
    }

    /// True when the on-disk log has at least one frame.
    pub fn has_data(&self) -> bool {
        self.write_pos > 0 || self.file.metadata().map(|m| m.len() > 0).unwrap_or(false)
    }

    /// Append a new object version and record it in the in-memory index.
    pub fn append_object(&mut self, v: &BinderObjectVersion) -> io::Result<()> {
        let offset = self.append_record(&VersionRecord::Object(v.clone()))?;
        self.object_index
            .entry(v.object_id.clone())
            .or_default()
            .push(IndexEntry {
                valid_from: v.valid_from,
                valid_to: v.valid_to,
                offset,
            });
        Ok(())
    }

    /// Append a new edge version and record it in the in-memory index.
    pub fn append_edge(&mut self, v: &BinderEdgeVersion) -> io::Result<()> {
        let offset = self.append_record(&VersionRecord::Edge(v.clone()))?;
        self.edge_index
            .entry(v.edge_id.clone())
            .or_default()
            .push(IndexEntry {
                valid_from: v.valid_from,
                valid_to: v.valid_to,
                offset,
            });
        Ok(())
    }

    /// Mark the most recent version of `id` as superseded by `retired_at`.
    /// Appends a `RetireObject` record and updates the in-memory index.
    pub fn retire_object(&mut self, id: &ObjectId, retired_at: CommitId) -> io::Result<()> {
        self.append_record(&VersionRecord::RetireObject {
            id: id.clone(),
            retired_at,
        })?;
        if let Some(entries) = self.object_index.get_mut(id) {
            if let Some(e) = entries.iter_mut().rfind(|e| e.valid_to.is_none()) {
                e.valid_to = Some(retired_at);
            }
        }
        Ok(())
    }

    /// Mark the most recent version of `id` as superseded by `retired_at`.
    pub fn retire_edge(&mut self, id: &EdgeId, retired_at: CommitId) -> io::Result<()> {
        self.append_record(&VersionRecord::RetireEdge {
            id: id.clone(),
            retired_at,
        })?;
        if let Some(entries) = self.edge_index.get_mut(id) {
            if let Some(e) = entries.iter_mut().rfind(|e| e.valid_to.is_none()) {
                e.valid_to = Some(retired_at);
            }
        }
        Ok(())
    }

    /// All versions of an object in ascending commit order.
    pub fn object_history(&mut self, id: &ObjectId) -> io::Result<Vec<BinderObjectVersion>> {
        let offsets: Vec<(u64, Option<CommitId>)> = self
            .object_index
            .get(id)
            .map(|e| e.iter().map(|x| (x.offset, x.valid_to)).collect())
            .unwrap_or_default();
        let mut result = Vec::with_capacity(offsets.len());
        for (offset, valid_to) in offsets {
            if let VersionRecord::Object(mut v) = self.read_record_at(offset)? {
                v.valid_to = valid_to;
                result.push(v);
            }
        }
        Ok(result)
    }

    /// Object version current at `at_commit`, or `None` if the object didn't
    /// exist (or was tombstoned) at that commit.
    pub fn object_as_of(
        &mut self,
        id: &ObjectId,
        at: CommitId,
    ) -> io::Result<Option<BinderObjectVersion>> {
        let entry = self
            .object_index
            .get(id)
            .and_then(|entries| {
                entries
                    .iter()
                    .rfind(|e| e.valid_from <= at && e.valid_to.is_none_or(|to| at < to))
            })
            .map(|e| (e.offset, e.valid_to));
        match entry {
            None => Ok(None),
            Some((offset, valid_to)) => {
                if let VersionRecord::Object(mut v) = self.read_record_at(offset)? {
                    v.valid_to = valid_to;
                    Ok(Some(v))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// All versions of an edge in ascending commit order.
    pub fn edge_history(&mut self, id: &EdgeId) -> io::Result<Vec<BinderEdgeVersion>> {
        let offsets: Vec<(u64, Option<CommitId>)> = self
            .edge_index
            .get(id)
            .map(|e| e.iter().map(|x| (x.offset, x.valid_to)).collect())
            .unwrap_or_default();
        let mut result = Vec::with_capacity(offsets.len());
        for (offset, valid_to) in offsets {
            if let VersionRecord::Edge(mut v) = self.read_record_at(offset)? {
                v.valid_to = valid_to;
                result.push(v);
            }
        }
        Ok(result)
    }

    /// Edge version current at `at_commit`.
    pub fn edge_as_of(
        &mut self,
        id: &EdgeId,
        at: CommitId,
    ) -> io::Result<Option<BinderEdgeVersion>> {
        let entry = self
            .edge_index
            .get(id)
            .and_then(|entries| {
                entries
                    .iter()
                    .rfind(|e| e.valid_from <= at && e.valid_to.is_none_or(|to| at < to))
            })
            .map(|e| (e.offset, e.valid_to));
        match entry {
            None => Ok(None),
            Some((offset, valid_to)) => {
                if let VersionRecord::Edge(mut v) = self.read_record_at(offset)? {
                    v.valid_to = valid_to;
                    Ok(Some(v))
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    // ── private helpers ───────────────────────────────────────────────────────

    fn append_record(&mut self, record: &VersionRecord) -> io::Result<u64> {
        let bytes = bincode::serialize(record).map_err(|e| io::Error::other(e.to_string()))?;
        let offset = self.write_pos;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        self.file.write_all(&bytes)?;
        self.write_pos += 8 + bytes.len() as u64;
        // Invalidate mmap — it no longer covers the full file.
        self.mmap = None;
        Ok(offset)
    }

    fn refresh_mmap(&mut self) -> io::Result<()> {
        if self.write_pos > 0 {
            self.mmap = Some(unsafe { Mmap::map(&self.file)? });
        } else {
            self.mmap = None;
        }
        Ok(())
    }

    fn read_record_at(&mut self, offset: u64) -> io::Result<VersionRecord> {
        // Lazily re-map after appends; use mmap slice instead of seek+read.
        if self.mmap.is_none() && self.write_pos > 0 {
            self.refresh_mmap()?;
        }
        if let Some(ref mmap) = self.mmap {
            let start = offset as usize;
            if start + 8 > mmap.len() {
                return Err(io::Error::other("version_store: offset out of bounds"));
            }
            let len = u64::from_le_bytes(mmap[start..start + 8].try_into().unwrap()) as usize;
            let data_start = start + 8;
            if data_start + len > mmap.len() {
                return Err(io::Error::other(
                    "version_store: frame extends past end of file",
                ));
            }
            return bincode::deserialize(&mmap[data_start..data_start + len])
                .map_err(|e| io::Error::other(e.to_string()));
        }
        // Fallback for empty file (shouldn't occur in normal use).
        self.file.seek(SeekFrom::Start(offset))?;
        let mut len_buf = [0u8; 8];
        self.file.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf)?;
        bincode::deserialize(&buf).map_err(|e| io::Error::other(e.to_string()))
    }
}
