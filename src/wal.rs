use crate::commit::BinderCommit;
use crate::error::StoreError;
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"BNDR";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WalEntryData {
    pub commit: BinderCommit,
}

pub(crate) struct Wal {
    #[allow(dead_code)]
    path: PathBuf,
    file: File,
    next_entry_id: u64,
}

impl Wal {
    pub fn open_or_create(path: &Path) -> Result<(Self, Vec<BinderCommit>), StoreError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let mut wal = Self {
            path: path.to_path_buf(),
            file,
            next_entry_id: 0,
        };

        let commits = wal.replay()?;
        Ok((wal, commits))
    }

    pub fn append(&mut self, commit: &BinderCommit) -> Result<(), StoreError> {
        self.write_frame(commit)?;
        self.fsync()?;
        Ok(())
    }

    /// Write the WAL frame without fsyncing. Callers must call `fsync()` before
    /// the data is durable. Used by `commit_buffered()` to coalesce fsyncs across
    /// many commits — trading per-commit durability for throughput.
    ///
    /// # Safety
    /// If the process crashes before `fsync()` is called, all commits written
    /// with `append_no_sync()` since the last fsync are lost. The dirty marker
    /// ensures WAL replay will discard partial frames on recovery.
    pub fn append_no_sync(&mut self, commit: &BinderCommit) -> Result<(), StoreError> {
        self.write_frame(commit)
    }

    fn write_frame(&mut self, commit: &BinderCommit) -> Result<(), StoreError> {
        let data = WalEntryData {
            commit: commit.clone(),
        };
        let encoded = bincode::serialize(&data)?;

        let mut checksum = Hasher::new();
        checksum.update(&encoded);
        let crc = checksum.finalize();

        let entry_id = self.next_entry_id;
        let data_len = encoded.len() as u64;

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(MAGIC)?;
        self.file.write_all(&entry_id.to_le_bytes())?;
        self.file.write_all(&data_len.to_le_bytes())?;
        self.file.write_all(&encoded)?;
        self.file.write_all(&crc.to_le_bytes())?;

        self.next_entry_id += 1;
        Ok(())
    }

    pub fn fsync(&mut self) -> Result<(), StoreError> {
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Current byte offset of the write position (end of last good entry).
    pub fn current_offset(&mut self) -> u64 {
        self.file.seek(SeekFrom::End(0)).unwrap_or(0)
    }

    /// Truncate the WAL file to zero bytes, preserving the current
    /// `next_entry_id` counter so future writes remain monotonically ordered.
    /// Called by `BinderStore::compact()` after a snapshot has been confirmed.
    pub fn truncate_to_start(&mut self) -> Result<(), StoreError> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Replay only the WAL frames at or after `start_offset`.
    /// Used by WAL compaction (issue #1): once frames before a checkpoint are
    /// trimmed, open() calls this to replay only the delta after the snapshot.
    #[allow(dead_code)]
    /// The snapshot covers everything before `start_offset`; this method
    /// recovers any commits written afterwards.
    ///
    /// Does NOT truncate the file before `start_offset` — those bytes belong
    /// to the snapshotted history and must remain intact for recovery.
    /// Truncates any partial/corrupted frames found after `start_offset`.
    pub fn replay_from(
        &mut self,
        start_offset: u64,
    ) -> Result<(Vec<BinderCommit>, u64), StoreError> {
        let file_len = self.file.metadata()?.len();
        if start_offset >= file_len {
            // Nothing to replay — WAL hasn't grown since the snapshot.
            self.file.seek(SeekFrom::End(0))?;
            return Ok((Vec::new(), start_offset));
        }

        self.file.seek(SeekFrom::Start(start_offset))?;
        let mut reader = BufReader::new(&self.file);
        let mut commits = Vec::new();
        let mut delta_good: u64 = 0;
        let mut next_entry_id: u64 = self.next_entry_id;

        loop {
            let mut magic = [0u8; 4];
            match reader.read_exact(&mut magic) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            if &magic != MAGIC {
                break;
            }

            let mut entry_id_bytes = [0u8; 8];
            if reader.read_exact(&mut entry_id_bytes).is_err() {
                break;
            }
            let entry_id = u64::from_le_bytes(entry_id_bytes);

            let mut data_len_bytes = [0u8; 8];
            if reader.read_exact(&mut data_len_bytes).is_err() {
                break;
            }
            let data_len = u64::from_le_bytes(data_len_bytes) as usize;

            let mut data = vec![0u8; data_len];
            if reader.read_exact(&mut data).is_err() {
                break;
            }

            let mut crc_bytes = [0u8; 4];
            if reader.read_exact(&mut crc_bytes).is_err() {
                break;
            }
            let stored_crc = u32::from_le_bytes(crc_bytes);

            let mut hasher = Hasher::new();
            hasher.update(&data);
            if hasher.finalize() != stored_crc {
                break;
            }

            let entry: WalEntryData = match bincode::deserialize(&data) {
                Ok(e) => e,
                Err(_) => break,
            };

            delta_good += 4 + 8 + 8 + data_len as u64 + 4;
            next_entry_id = entry_id + 1;
            commits.push(entry.commit);
        }

        // Truncate any partial frames after the last good delta frame.
        let good_total = start_offset + delta_good;
        self.file.set_len(good_total)?;
        self.file.seek(SeekFrom::End(0))?;
        self.next_entry_id = next_entry_id;

        Ok((commits, good_total))
    }

    /// Replay all valid WAL entries from the beginning. Stops at the first corrupted or
    /// incomplete entry and truncates the file there. Returns all recovered commits.
    fn replay(&mut self) -> Result<Vec<BinderCommit>, StoreError> {
        self.file.seek(SeekFrom::Start(0))?;
        let file_len = self.file.metadata()?.len();

        if file_len == 0 {
            return Ok(Vec::new());
        }

        let mut reader = BufReader::new(&self.file);
        let mut commits = Vec::new();
        let mut good_offset: u64 = 0;
        let mut next_entry_id: u64 = 0;

        loop {
            let mut magic = [0u8; 4];
            match reader.read_exact(&mut magic) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }

            if &magic != MAGIC {
                break;
            }

            let mut entry_id_bytes = [0u8; 8];
            if reader.read_exact(&mut entry_id_bytes).is_err() {
                break;
            }
            let entry_id = u64::from_le_bytes(entry_id_bytes);

            let mut data_len_bytes = [0u8; 8];
            if reader.read_exact(&mut data_len_bytes).is_err() {
                break;
            }
            let data_len = u64::from_le_bytes(data_len_bytes) as usize;

            let mut data = vec![0u8; data_len];
            if reader.read_exact(&mut data).is_err() {
                break;
            }

            let mut crc_bytes = [0u8; 4];
            if reader.read_exact(&mut crc_bytes).is_err() {
                break;
            }
            let stored_crc = u32::from_le_bytes(crc_bytes);

            let mut hasher = Hasher::new();
            hasher.update(&data);
            let computed_crc = hasher.finalize();

            if computed_crc != stored_crc {
                break;
            }

            let entry: WalEntryData = match bincode::deserialize(&data) {
                Ok(e) => e,
                Err(_) => break,
            };

            good_offset += 4 + 8 + 8 + data_len as u64 + 4;
            next_entry_id = entry_id + 1;
            commits.push(entry.commit);
        }

        // Truncate to the last good entry so future appends don't write after a corrupted frame.
        self.file.set_len(good_offset)?;
        self.file.seek(SeekFrom::End(0))?;
        self.next_entry_id = next_entry_id;

        Ok(commits)
    }
}
