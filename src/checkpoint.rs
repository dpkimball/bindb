use crate::error::StoreError;
use crc32fast::Hasher;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Checkpoint record on disk: wal_offset + commit_count + crc32.
/// Total: 20 bytes per record. Multiple records may be appended; the last
/// valid one wins. Reading stops at the first bad checksum.
const RECORD_SIZE: usize = 20;

#[derive(Debug, Clone, Copy)]
pub struct CheckpointRecord {
    /// Byte offset in the WAL file after the last checkpointed commit.
    pub wal_offset: u64,
    /// Number of commits represented up to this checkpoint.
    pub commit_count: u64,
}

pub struct CheckpointLog {
    #[allow(dead_code)]
    path: PathBuf,
    file: File,
}

impl CheckpointLog {
    pub fn open_or_create(path: &Path) -> Result<Self, StoreError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Read the last valid checkpoint from the log, if any.
    pub fn last(&mut self) -> Result<Option<CheckpointRecord>, StoreError> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf)?;

        let mut last: Option<CheckpointRecord> = None;
        let chunks = buf.chunks_exact(RECORD_SIZE);

        for chunk in chunks {
            let wal_offset = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
            let commit_count = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
            let stored_crc = u32::from_le_bytes(chunk[16..20].try_into().unwrap());

            let mut hasher = Hasher::new();
            hasher.update(&chunk[0..16]);
            if hasher.finalize() == stored_crc {
                last = Some(CheckpointRecord {
                    wal_offset,
                    commit_count,
                });
            }
        }

        Ok(last)
    }

    /// Append a new checkpoint record. Returns the written record.
    pub fn write(&mut self, record: CheckpointRecord) -> Result<(), StoreError> {
        let mut buf = [0u8; RECORD_SIZE];
        buf[0..8].copy_from_slice(&record.wal_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&record.commit_count.to_le_bytes());

        let mut hasher = Hasher::new();
        hasher.update(&buf[0..16]);
        let crc = hasher.finalize();
        buf[16..20].copy_from_slice(&crc.to_le_bytes());

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&buf)?;
        self.file.sync_all()?;
        Ok(())
    }
}
