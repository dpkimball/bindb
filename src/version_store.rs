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
/// ### Cold-version eviction (issue #6)
/// When `total_index_entries` exceeds `max_history_entries` (default: 1 M),
/// all historical IndexEntries (`valid_to = Some(…)`) are dropped from RAM
/// while live entries (`valid_to = None`) are kept.  Affected IDs are recorded
/// in `evicted_object_ids` / `evicted_edge_ids`.  Temporal queries
/// (`object_history`, `object_as_of`, `edge_history`, `edge_as_of`) for evicted
/// IDs fall back to a mmap scan of `version_log.bin` — O(file_size) but
/// page-cache-backed, and only on the cold query path.
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
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

// ── type aliases ─────────────────────────────────────────────────────────────

/// Full original structs recovered from mmap before rebuild truncates the file.
/// Keyed by (id, valid_from) which is the stable identity of a version.
type ObjRecoveryMap = HashMap<(ObjectId, CommitId), BinderObjectVersion>;
type EdgeRecoveryMap = HashMap<(EdgeId, CommitId), BinderEdgeVersion>;

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

// ── default eviction cap ──────────────────────────────────────────────────────

/// Default maximum number of IndexEntry records held in RAM before historical
/// (non-live) entries are evicted.  At ~40 B each this is ~40 MB.
const DEFAULT_MAX_HISTORY_ENTRIES: usize = 1_000_000;

// ── public struct ─────────────────────────────────────────────────────────────

pub struct VersionStore {
    file: File,
    write_pos: u64,
    /// Lazily refreshed read mapping. None when dirty (after an append).
    mmap: Option<Mmap>,
    object_index: HashMap<ObjectId, Vec<IndexEntry>>,
    edge_index: HashMap<EdgeId, Vec<IndexEntry>>,
    /// Running count of all IndexEntry records across both indexes.
    total_index_entries: usize,
    /// Cap above which `evict_cold_history()` is triggered.
    max_history_entries: usize,
    /// IDs whose historical (non-live) index entries have been evicted.
    /// Temporal queries for these IDs fall back to a mmap scan.
    evicted_object_ids: HashSet<ObjectId>,
    evicted_edge_ids: HashSet<EdgeId>,
}

impl VersionStore {
    pub fn open_or_create(path: &Path) -> io::Result<Self> {
        Self::open_or_create_with_cap(path, DEFAULT_MAX_HISTORY_ENTRIES)
    }

    /// Like `open_or_create` but with a custom eviction cap.  Used by tests
    /// and internal code that needs a lower cap for deterministic eviction.
    pub(crate) fn open_or_create_with_cap(
        path: &Path,
        max_history_entries: usize,
    ) -> io::Result<Self> {
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
            total_index_entries: 0,
            max_history_entries,
            evicted_object_ids: HashSet::new(),
            evicted_edge_ids: HashSet::new(),
        })
    }

    /// Truncate the file and rebuild it from the given version slices.
    ///
    /// Called during `open()` (after WAL replay) and `rebuild_indexes()`.
    ///
    /// Entries with `payload_stripped = true` have had payload, version_id,
    /// commit_id, and provenance zeroed from RAM.  Their original values are
    /// recovered from the existing mmap before truncation so the rebuilt file
    /// is always complete.
    pub fn rebuild_from_versions(
        &mut self,
        objects: &[BinderObjectVersion],
        edges: &[BinderEdgeVersion],
    ) -> io::Result<()> {
        // Recover full structs for any compacted entries before truncating.
        let obj_originals = self.recover_stripped_obj_records(objects)?;
        let edge_originals = self.recover_stripped_edge_records(edges)?;

        self.mmap = None;
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.write_pos = 0;
        self.object_index.clear();
        self.edge_index.clear();
        self.total_index_entries = 0;
        self.evicted_object_ids.clear();
        self.evicted_edge_ids.clear();

        for v in objects {
            let full_v = if v.payload_stripped {
                let key = (v.object_id.clone(), v.valid_from);
                match obj_originals.get(&key) {
                    Some(original) => original.clone(),
                    None => {
                        // No disk record found; write the compacted version as-is.
                        let mut r = v.clone();
                        r.payload_stripped = false;
                        r
                    }
                }
            } else {
                v.clone()
            };
            let offset = self.append_record(&VersionRecord::Object(full_v))?;
            self.object_index
                .entry(v.object_id.clone())
                .or_default()
                .push(IndexEntry {
                    valid_from: v.valid_from,
                    valid_to: v.valid_to,
                    offset,
                });
            if v.valid_to.is_some() {
                self.total_index_entries += 1;
            }
        }

        for v in edges {
            let full_v = if v.payload_stripped {
                let key = (v.edge_id.clone(), v.valid_from);
                match edge_originals.get(&key) {
                    Some(original) => original.clone(),
                    None => {
                        let mut r = v.clone();
                        r.payload_stripped = false;
                        r
                    }
                }
            } else {
                v.clone()
            };
            let offset = self.append_record(&VersionRecord::Edge(full_v))?;
            self.edge_index
                .entry(v.edge_id.clone())
                .or_default()
                .push(IndexEntry {
                    valid_from: v.valid_from,
                    valid_to: v.valid_to,
                    offset,
                });
            if v.valid_to.is_some() {
                self.total_index_entries += 1;
            }
        }

        self.file.flush()?;
        self.refresh_mmap()?;
        self.maybe_evict();
        Ok(())
    }

    /// Scan the existing mmap and return the original full structs for any
    /// compacted (payload_stripped=true) object versions, keyed by (object_id, valid_from).
    fn recover_stripped_obj_records(
        &mut self,
        objects: &[BinderObjectVersion],
    ) -> io::Result<ObjRecoveryMap> {
        let mut map = ObjRecoveryMap::new();
        if !objects.iter().any(|v| v.payload_stripped) {
            return Ok(map);
        }
        if self.mmap.is_none() && self.write_pos > 0 {
            self.refresh_mmap()?;
        }
        let Some(ref mmap) = self.mmap else {
            return Ok(map);
        };
        let mut pos = 0usize;
        while pos + 8 <= mmap.len() {
            let len = u64::from_le_bytes(mmap[pos..pos + 8].try_into().unwrap()) as usize;
            let data_start = pos + 8;
            if data_start + len > mmap.len() {
                break;
            }
            if let Ok(VersionRecord::Object(v)) =
                bincode::deserialize::<VersionRecord>(&mmap[data_start..data_start + len])
            {
                map.entry((v.object_id.clone(), v.valid_from)).or_insert(v);
            }
            pos += 8 + len;
        }
        Ok(map)
    }

    /// Scan the existing mmap and return the original full structs for any
    /// compacted edge versions, keyed by (edge_id, valid_from).
    fn recover_stripped_edge_records(
        &mut self,
        edges: &[BinderEdgeVersion],
    ) -> io::Result<EdgeRecoveryMap> {
        let mut map = EdgeRecoveryMap::new();
        if !edges.iter().any(|v| v.payload_stripped) {
            return Ok(map);
        }
        if self.mmap.is_none() && self.write_pos > 0 {
            self.refresh_mmap()?;
        }
        let Some(ref mmap) = self.mmap else {
            return Ok(map);
        };
        let mut pos = 0usize;
        while pos + 8 <= mmap.len() {
            let len = u64::from_le_bytes(mmap[pos..pos + 8].try_into().unwrap()) as usize;
            let data_start = pos + 8;
            if data_start + len > mmap.len() {
                break;
            }
            if let Ok(VersionRecord::Edge(v)) =
                bincode::deserialize::<VersionRecord>(&mmap[data_start..data_start + len])
            {
                map.entry((v.edge_id.clone(), v.valid_from)).or_insert(v);
            }
            pos += 8 + len;
        }
        Ok(map)
    }

    /// Rebuild in-memory indexes by scanning an existing `version_log.bin`
    /// without rewriting it. Used on clean reopen when the file is already
    /// consistent with the snapshot (avoids O(n) rewrite on every `open()`).
    pub fn hydrate_index_from_file(&mut self) -> io::Result<()> {
        self.object_index.clear();
        self.edge_index.clear();
        self.total_index_entries = 0;
        self.evicted_object_ids.clear();
        self.evicted_edge_ids.clear();
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
                    let historical = v.valid_to.is_some();
                    self.object_index
                        .entry(v.object_id)
                        .or_default()
                        .push(IndexEntry {
                            valid_from: v.valid_from,
                            valid_to: v.valid_to,
                            offset,
                        });
                    if historical {
                        self.total_index_entries += 1;
                    }
                }
                VersionRecord::Edge(v) => {
                    let historical = v.valid_to.is_some();
                    self.edge_index
                        .entry(v.edge_id)
                        .or_default()
                        .push(IndexEntry {
                            valid_from: v.valid_from,
                            valid_to: v.valid_to,
                            offset,
                        });
                    if historical {
                        self.total_index_entries += 1;
                    }
                }
                VersionRecord::RetireObject { id, retired_at } => {
                    if let Some(entries) = self.object_index.get_mut(&id) {
                        if let Some(e) = entries.iter_mut().rfind(|e| e.valid_to.is_none()) {
                            e.valid_to = Some(retired_at);
                            self.total_index_entries += 1;
                        }
                    }
                }
                VersionRecord::RetireEdge { id, retired_at } => {
                    if let Some(entries) = self.edge_index.get_mut(&id) {
                        if let Some(e) = entries.iter_mut().rfind(|e| e.valid_to.is_none()) {
                            e.valid_to = Some(retired_at);
                            self.total_index_entries += 1;
                        }
                    }
                }
            }
        }

        self.write_pos = pos;
        // Truncate any partial write at the tail (crash recovery).
        self.file.set_len(pos)?;
        self.file.seek(SeekFrom::Start(self.write_pos))?;
        self.refresh_mmap()?;
        self.maybe_evict();
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

    /// Check whether a specific version (identified by valid_from) exists in the index for an object.
    pub fn object_has_version_at(&self, id: &ObjectId, valid_from: CommitId) -> bool {
        self.object_index
            .get(id)
            .is_some_and(|entries| entries.iter().any(|e| e.valid_from == valid_from))
    }

    /// Check whether a specific version (identified by valid_from) exists in the index for an edge.
    pub fn edge_has_version_at(&self, id: &EdgeId, valid_from: CommitId) -> bool {
        self.edge_index
            .get(id)
            .is_some_and(|entries| entries.iter().any(|e| e.valid_from == valid_from))
    }

    /// Current number of IndexEntry records held in RAM (live + historical).
    #[allow(dead_code)]
    pub fn index_entry_count(&self) -> usize {
        self.total_index_entries
    }

    /// Number of object IDs whose historical entries have been evicted.
    #[allow(dead_code)]
    pub fn evicted_object_count(&self) -> usize {
        self.evicted_object_ids.len()
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
        // Only historical (valid_to = Some) entries count toward the eviction
        // cap. Live entries cannot be freed, so counting them would cause
        // spurious O(N) eviction sweeps that drop nothing.
        if v.valid_to.is_some() {
            self.total_index_entries += 1;
            self.maybe_evict();
        }
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
        if v.valid_to.is_some() {
            self.total_index_entries += 1;
            self.maybe_evict();
        }
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
                // The entry just became historical — count it now.
                self.total_index_entries += 1;
                self.maybe_evict();
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
                self.total_index_entries += 1;
                self.maybe_evict();
            }
        }
        Ok(())
    }

    /// Current (head) version of an object, read from the mmap'd log.
    /// Returns `None` when the object is unknown to this VersionStore.
    /// Used by the engine as a fallback when the head cache has evicted the entry.
    pub fn get_head_object(&mut self, id: &ObjectId) -> io::Result<Option<BinderObjectVersion>> {
        if self.evicted_object_ids.contains(id) {
            let history = self.scan_object_history_from_mmap(id)?;
            return Ok(history.into_iter().find(|v| v.valid_to.is_none()));
        }
        let entry = self
            .object_index
            .get(id)
            .and_then(|entries| entries.iter().rfind(|e| e.valid_to.is_none()))
            .map(|e| e.offset);
        match entry {
            None => Ok(None),
            Some(offset) => match self.read_record_at(offset)? {
                VersionRecord::Object(mut v) => {
                    v.valid_to = None;
                    Ok(Some(v))
                }
                _ => Ok(None),
            },
        }
    }

    /// Current (head) version of an edge, read from the mmap'd log.
    pub fn get_head_edge(&mut self, id: &EdgeId) -> io::Result<Option<BinderEdgeVersion>> {
        if self.evicted_edge_ids.contains(id) {
            let history = self.scan_edge_history_from_mmap(id)?;
            return Ok(history.into_iter().find(|v| v.valid_to.is_none()));
        }
        let entry = self
            .edge_index
            .get(id)
            .and_then(|entries| entries.iter().rfind(|e| e.valid_to.is_none()))
            .map(|e| e.offset);
        match entry {
            None => Ok(None),
            Some(offset) => match self.read_record_at(offset)? {
                VersionRecord::Edge(mut v) => {
                    v.valid_to = None;
                    Ok(Some(v))
                }
                _ => Ok(None),
            },
        }
    }

    /// All versions of an object in ascending commit order.
    pub fn object_history(&mut self, id: &ObjectId) -> io::Result<Vec<BinderObjectVersion>> {
        if self.evicted_object_ids.contains(id) {
            return self.scan_object_history_from_mmap(id);
        }
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
        if self.evicted_object_ids.contains(id) {
            let history = self.scan_object_history_from_mmap(id)?;
            return Ok(history
                .into_iter()
                .rfind(|v| v.valid_from <= at && v.valid_to.is_none_or(|to| at < to)));
        }
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
        if self.evicted_edge_ids.contains(id) {
            return self.scan_edge_history_from_mmap(id);
        }
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
        if self.evicted_edge_ids.contains(id) {
            let history = self.scan_edge_history_from_mmap(id)?;
            return Ok(history
                .into_iter()
                .rfind(|v| v.valid_from <= at && v.valid_to.is_none_or(|to| at < to)));
        }
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

    /// Drop all historical (valid_to = Some) IndexEntry records from RAM.
    /// Live entries (valid_to = None) are kept so retire / append remain correct.
    /// Affected IDs are added to `evicted_object_ids` / `evicted_edge_ids`.
    fn evict_cold_history(&mut self) {
        for (id, entries) in &mut self.object_index {
            let before = entries.len();
            entries.retain(|e| e.valid_to.is_none());
            let dropped = before - entries.len();
            if dropped > 0 {
                self.total_index_entries -= dropped;
                self.evicted_object_ids.insert(id.clone());
            }
        }
        for (id, entries) in &mut self.edge_index {
            let before = entries.len();
            entries.retain(|e| e.valid_to.is_none());
            let dropped = before - entries.len();
            if dropped > 0 {
                self.total_index_entries -= dropped;
                self.evicted_edge_ids.insert(id.clone());
            }
        }
    }

    fn maybe_evict(&mut self) {
        if self.total_index_entries > self.max_history_entries {
            self.evict_cold_history();
        }
    }

    /// Scan the mmap'd `version_log.bin` to rebuild the full history for `id`.
    /// Used when the ID's historical index entries have been evicted.
    fn scan_object_history_from_mmap(
        &mut self,
        id: &ObjectId,
    ) -> io::Result<Vec<BinderObjectVersion>> {
        if self.mmap.is_none() && self.write_pos > 0 {
            self.refresh_mmap()?;
        }
        let Some(ref mmap) = self.mmap else {
            return Ok(vec![]);
        };
        // Borrow the mmap slice for the scan.
        let mut versions: Vec<BinderObjectVersion> = Vec::new();
        let mut pos = 0usize;
        while pos + 8 <= mmap.len() {
            let len = u64::from_le_bytes(mmap[pos..pos + 8].try_into().unwrap()) as usize;
            let data_start = pos + 8;
            if data_start + len > mmap.len() {
                break;
            }
            let record: VersionRecord =
                match bincode::deserialize(&mmap[data_start..data_start + len]) {
                    Ok(r) => r,
                    Err(_) => break,
                };
            match record {
                VersionRecord::Object(v) if &v.object_id == id => {
                    versions.push(v);
                }
                VersionRecord::RetireObject {
                    id: ref rid,
                    retired_at,
                } if rid == id => {
                    if let Some(v) = versions.iter_mut().rfind(|v| v.valid_to.is_none()) {
                        v.valid_to = Some(retired_at);
                    }
                }
                _ => {}
            }
            pos += 8 + len;
        }
        Ok(versions)
    }

    /// Same as `scan_object_history_from_mmap` but for edges.
    fn scan_edge_history_from_mmap(&mut self, id: &EdgeId) -> io::Result<Vec<BinderEdgeVersion>> {
        if self.mmap.is_none() && self.write_pos > 0 {
            self.refresh_mmap()?;
        }
        let Some(ref mmap) = self.mmap else {
            return Ok(vec![]);
        };
        let mut versions: Vec<BinderEdgeVersion> = Vec::new();
        let mut pos = 0usize;
        while pos + 8 <= mmap.len() {
            let len = u64::from_le_bytes(mmap[pos..pos + 8].try_into().unwrap()) as usize;
            let data_start = pos + 8;
            if data_start + len > mmap.len() {
                break;
            }
            let record: VersionRecord =
                match bincode::deserialize(&mmap[data_start..data_start + len]) {
                    Ok(r) => r,
                    Err(_) => break,
                };
            match record {
                VersionRecord::Edge(v) if &v.edge_id == id => {
                    versions.push(v);
                }
                VersionRecord::RetireEdge {
                    id: ref rid,
                    retired_at,
                } if rid == id => {
                    if let Some(v) = versions.iter_mut().rfind(|v| v.valid_to.is_none()) {
                        v.valid_to = Some(retired_at);
                    }
                }
                _ => {}
            }
            pos += 8 + len;
        }
        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{BinderObjectVersion, ObjectVersionId};
    use tempfile::tempdir;
    use ulid::Ulid;

    fn cid(n: u128) -> CommitId {
        CommitId(Ulid(n))
    }

    fn make_obj_version(
        id: &str,
        payload: &[u8],
        valid_from: CommitId,
        valid_to: Option<CommitId>,
    ) -> BinderObjectVersion {
        BinderObjectVersion {
            version_id: ObjectVersionId(Ulid::nil()),
            object_id: ObjectId::new(id),
            commit_id: valid_from,
            valid_from,
            valid_to,
            payload: Some(payload.to_vec()),
            provenance: None,
            payload_stripped: false,
        }
    }

    #[test]
    fn eviction_drops_historical_entries_and_keeps_live() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vs.bin");
        // Cap of 2 historical entries. Only retired (valid_to=Some) entries count
        // toward the cap; live entries do not. Eviction fires when historical > cap.
        let mut vs = VersionStore::open_or_create_with_cap(&path, 2).unwrap();

        // Write 4 versions of object "x" — v1 → v2 → v3 → v4.
        // v1/v2/v3 are appended pre-retired (valid_to already set).
        let v1 = make_obj_version("x", b"v1", cid(1), Some(cid(2)));
        let v2 = make_obj_version("x", b"v2", cid(2), Some(cid(3)));
        let v3 = make_obj_version("x", b"v3", cid(3), Some(cid(4)));
        let v4 = make_obj_version("x", b"v4", cid(4), None);

        // Each historical append increments the counter; eviction fires at > 2.
        vs.append_object(&v1).unwrap(); // historical count = 1
        vs.append_object(&v2).unwrap(); // historical count = 2
        vs.append_object(&v3).unwrap(); // historical count = 3 → eviction fires here

        // After eviction x should be in evicted_object_ids.
        assert!(vs.evicted_object_ids.contains(&ObjectId::new("x")));
        assert_eq!(vs.evicted_object_count(), 1);

        // Appending the live version does not affect the historical count.
        vs.append_object(&v4).unwrap();
        let in_mem = vs.object_index.get(&ObjectId::new("x")).unwrap();
        assert_eq!(in_mem.len(), 1, "only live entry should remain in memory");
        assert!(in_mem[0].valid_to.is_none(), "surviving entry must be live");
    }

    #[test]
    fn evicted_id_history_recovered_via_mmap_scan() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vs.bin");
        // Cap of 2 historical entries; eviction fires when a 3rd historical is added.
        let mut vs = VersionStore::open_or_create_with_cap(&path, 2).unwrap();

        let id = ObjectId::new("y");
        for i in 0u128..4 {
            let valid_to = if i < 3 { Some(cid(i + 2)) } else { None };
            let v = make_obj_version("y", format!("v{i}").as_bytes(), cid(i + 1), valid_to);
            if i > 0 {
                vs.retire_object(&id, cid(i + 1)).unwrap();
            }
            vs.append_object(&v).unwrap();
        }

        // y must have been evicted.
        assert!(vs.evicted_object_ids.contains(&id));

        // Full history must still be recoverable.
        let history = vs.object_history(&id).unwrap();
        assert_eq!(history.len(), 4);
        assert!(history.last().unwrap().valid_to.is_none());

        // as_of at commit 2 should return v1 (valid_from=1, valid_to=2).
        let snap = vs.object_as_of(&id, cid(1)).unwrap();
        assert!(snap.is_some());
        assert_eq!(snap.unwrap().payload, Some(b"v0".to_vec()));
    }

    #[test]
    fn index_entry_count_tracks_correctly() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vs.bin");
        let mut vs = VersionStore::open_or_create_with_cap(&path, 1_000_000).unwrap();

        let id = ObjectId::new("z");
        assert_eq!(vs.index_entry_count(), 0, "fresh store has no entries");

        // Appending a live entry does NOT count — only historical entries do.
        let v1 = make_obj_version("z", b"a", cid(1), None);
        vs.append_object(&v1).unwrap();
        assert_eq!(vs.index_entry_count(), 0, "live entry is not counted");

        // Retiring v1 makes it historical — now it counts.
        vs.retire_object(&id, cid(2)).unwrap();
        assert_eq!(
            vs.index_entry_count(),
            1,
            "retired entry counted as historical"
        );

        // Appending a new live version still doesn't increase the historical count.
        let v2 = make_obj_version("z", b"b", cid(2), None);
        vs.append_object(&v2).unwrap();
        assert_eq!(vs.index_entry_count(), 1, "second live entry not counted");
    }
}
