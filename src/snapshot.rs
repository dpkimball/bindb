/// Version store snapshot — written on clean close, loaded on open.
///
/// Avoids full WAL replay on `open()`: instead of deserializing every WAL
/// frame from byte 0, we load the pre-built version Vecs and position maps
/// directly from the snapshot, then replay only the delta (WAL frames written
/// after the snapshot's `wal_offset`).
///
/// Format: MAGIC(4) + version(1) + payload_len(8) + payload(N) + crc32(4)
/// The CRC covers only the payload bytes.  The version byte allows future
/// format changes without a separate magic number.
use crate::edge::BinderEdgeVersion;
use crate::error::StoreError;
use crate::object::BinderObjectVersion;
use crate::types::{EdgeId, ObjectId};
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const MAGIC: &[u8; 4] = b"BSNA";
const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
pub struct VersionSnapshot {
    pub commit_count: u64,
    pub wal_offset: u64,
    pub object_versions: Vec<BinderObjectVersion>,
    pub edge_versions: Vec<BinderEdgeVersion>,
    pub ov_positions: HashMap<ObjectId, usize>,
    pub ev_positions: HashMap<EdgeId, usize>,
}

pub fn write_snapshot(path: &Path, snapshot: &VersionSnapshot) -> Result<(), StoreError> {
    let payload = bincode::serialize(snapshot)?;
    let payload_len = payload.len() as u64;

    let mut hasher = Hasher::new();
    hasher.update(&payload);
    let crc = hasher.finalize();

    let mut buf =
        Vec::with_capacity(4 + 1 + 8 + payload.len() + 4);
    buf.extend_from_slice(MAGIC);
    buf.push(VERSION);
    buf.extend_from_slice(&payload_len.to_le_bytes());
    buf.extend_from_slice(&payload);
    buf.extend_from_slice(&crc.to_le_bytes());

    fs::write(path, &buf)?;
    Ok(())
}

pub fn read_snapshot(path: &Path) -> Option<VersionSnapshot> {
    let data = fs::read(path).ok()?;

    if data.len() < 4 + 1 + 8 + 4 {
        return None;
    }

    if &data[0..4] != MAGIC {
        return None;
    }

    if data[4] != VERSION {
        return None;
    }

    let payload_len = u64::from_le_bytes(data[5..13].try_into().ok()?) as usize;
    let header_len = 4 + 1 + 8;
    let footer_len = 4;

    if data.len() < header_len + payload_len + footer_len {
        return None;
    }

    let payload = &data[header_len..header_len + payload_len];
    let stored_crc = u32::from_le_bytes(
        data[header_len + payload_len..header_len + payload_len + 4]
            .try_into()
            .ok()?,
    );

    let mut hasher = Hasher::new();
    hasher.update(payload);
    if hasher.finalize() != stored_crc {
        return None;
    }

    bincode::deserialize(payload).ok()
}
