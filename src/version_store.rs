/// Disk-backed version history store.
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
        Ok(Self {
            file,
            write_pos,
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

        self.file.flush()
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
        Ok(offset)
    }

    fn read_record_at(&mut self, offset: u64) -> io::Result<VersionRecord> {
        self.file.seek(SeekFrom::Start(offset))?;
        let mut len_buf = [0u8; 8];
        self.file.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf)?;
        bincode::deserialize(&buf).map_err(|e| io::Error::other(e.to_string()))
    }
}
