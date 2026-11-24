//! `.dcraftpkg` complete workspace package archive (active notebooks, trash, and preferences).

use std::io::{Read, Write};

use drawcraft_doc::{Document, UserStudioConfig};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};

use crate::{FormatError, binary};

/// Magic header written by DrawCraft (`.dcraftpkg`).
pub const PACKAGE_MAGIC: &[u8; 4] = b"DCPK";
/// Legacy magic of PaperCraft `.pcraftpkg` archives; still readable.
pub const LEGACY_PACKAGE_MAGIC: &[u8; 4] = b"PCPK";
/// Legacy magic of NoteCraft `.ncraftpkg` archives; still readable.
pub const NOTECRAFT_PACKAGE_MAGIC: &[u8; 4] = b"NCPK";
/// Magic of pre-release `.scraftpkg` archives (the project's earlier name); still readable.
pub const EARLY_PACKAGE_MAGIC: &[u8; 4] = b"SCPK";

fn has_package_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(PACKAGE_MAGIC)
        || bytes.starts_with(LEGACY_PACKAGE_MAGIC)
        || bytes.starts_with(NOTECRAFT_PACKAGE_MAGIC)
        || bytes.starts_with(EARLY_PACKAGE_MAGIC)
}
pub const PACKAGE_VERSION: u16 = 1;
const PACKAGE_HEADER_LEN: usize = 8;

const MIN_DECOMPRESSED_BYTES_CAP: usize = 128 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES_CAP: usize = 512 * 1024 * 1024;
const MAX_DECOMPRESSION_RATIO: usize = 64;

pub const MAX_PACKAGE_NOTEBOOKS: usize = 1024;
pub const MAX_PACKAGE_TRASHED: usize = 1024;
const MAX_SOURCE_NAME_BYTES: usize = 1024;
const MAX_PREFS_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_PACKAGE_ITEM_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct PackagedTrashEntry {
    pub deleted_at_ms: i64,
    pub document: Document,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspacePackage {
    pub exported_at_ms: i64,
    pub source_name: String,
    pub prefs: UserStudioConfig,
    pub notebooks: Vec<Document>,
    pub trashed_notebooks: Vec<PackagedTrashEntry>,
}

/// Returns true when `bytes` begins with the `.dcraftpkg` (or legacy `.ncraftpkg`) magic header.
#[must_use]
pub fn looks_like_package_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= PACKAGE_HEADER_LEN && has_package_magic(bytes)
}

/// Serializes an entire workspace (active notebooks, trash entries, and studio config)
/// into compressed `.dcraftpkg` v1 binary format.
pub fn save_package_bytes(pkg: &WorkspacePackage) -> Result<Vec<u8>, FormatError> {
    if pkg.notebooks.len() > MAX_PACKAGE_NOTEBOOKS {
        return Err(FormatError::PayloadTooLarge {
            limit_bytes: MAX_PACKAGE_NOTEBOOKS,
        });
    }
    if pkg.trashed_notebooks.len() > MAX_PACKAGE_TRASHED {
        return Err(FormatError::PayloadTooLarge {
            limit_bytes: MAX_PACKAGE_TRASHED,
        });
    }

    let mut raw = Vec::with_capacity(4096);
    raw.extend_from_slice(&pkg.exported_at_ms.to_le_bytes());

    let clean_source: String = pkg
        .source_name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(256)
        .collect();
    write_len_bytes(&mut raw, clean_source.as_bytes(), MAX_SOURCE_NAME_BYTES)?;

    let prefs_json = serde_json::to_vec(&pkg.prefs)?;
    write_len_bytes(&mut raw, &prefs_json, MAX_PREFS_JSON_BYTES)?;

    write_u32(&mut raw, pkg.notebooks.len())?;
    for doc in &pkg.notebooks {
        let doc_bytes = binary::encode_binary_v2(doc)?;
        write_len_bytes(&mut raw, &doc_bytes, MAX_PACKAGE_ITEM_BYTES)?;
    }

    write_u32(&mut raw, pkg.trashed_notebooks.len())?;
    for entry in &pkg.trashed_notebooks {
        raw.extend_from_slice(&entry.deleted_at_ms.to_le_bytes());
        let doc_bytes = binary::encode_binary_v2(&entry.document)?;
        write_len_bytes(&mut raw, &doc_bytes, MAX_PACKAGE_ITEM_BYTES)?;
    }

    let mut out = Vec::with_capacity(PACKAGE_HEADER_LEN + raw.len() / 2);
    out.extend_from_slice(PACKAGE_MAGIC);
    out.extend_from_slice(&PACKAGE_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    let mut encoder = ZlibEncoder::new(out, Compression::fast());
    encoder
        .write_all(&raw)
        .map_err(|e| FormatError::Compression(e.to_string()))?;
    encoder
        .finish()
        .map_err(|e| FormatError::Compression(e.to_string()))
}

/// Decodes and validates a `.dcraftpkg` (or legacy `.ncraftpkg`) v1 workspace archive.
pub fn load_package_bytes(bytes: &[u8]) -> Result<WorkspacePackage, FormatError> {
    if bytes.len() < PACKAGE_HEADER_LEN {
        return Err(FormatError::BinaryCorrupt(
            "package file truncated before header".to_string(),
        ));
    }
    if !has_package_magic(bytes) {
        return Err(FormatError::InvalidFormat(
            "missing .dcraftpkg magic header".to_string(),
        ));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != PACKAGE_VERSION {
        return Err(FormatError::UnsupportedVersion(u32::from(version)));
    }
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    if flags != 0 {
        return Err(FormatError::UnsupportedVersion(u32::from(flags)));
    }

    let payload = decompress_bounded(&bytes[PACKAGE_HEADER_LEN..])?;
    let mut reader = SliceReader {
        data: &payload,
        pos: 0,
    };

    let exported_at_ms = reader.read_i64()?;
    let source_bytes = reader.read_len_bytes(MAX_SOURCE_NAME_BYTES)?;
    let source_name = String::from_utf8(source_bytes.to_vec())
        .map_err(|_| FormatError::BinaryCorrupt("invalid UTF-8 package source name".to_string()))?;

    let prefs_bytes = reader.read_len_bytes(MAX_PREFS_JSON_BYTES)?;
    let mut prefs: UserStudioConfig = serde_json::from_slice(prefs_bytes)?;
    prefs.ui_scale = UserStudioConfig::sanitize_ui_scale(prefs.ui_scale);

    let nb_count = reader.read_count(MAX_PACKAGE_NOTEBOOKS, "notebooks")?;
    let mut notebooks = Vec::with_capacity(nb_count.min(256));
    for _ in 0..nb_count {
        let item_bytes = reader.read_len_bytes(MAX_PACKAGE_ITEM_BYTES)?;
        notebooks.push(crate::load_document_bytes(item_bytes)?);
    }

    let trash_count = reader.read_count(MAX_PACKAGE_TRASHED, "trashed notebooks")?;
    let mut trashed_notebooks = Vec::with_capacity(trash_count.min(256));
    for _ in 0..trash_count {
        let deleted_at_ms = reader.read_i64()?;
        let item_bytes = reader.read_len_bytes(MAX_PACKAGE_ITEM_BYTES)?;
        trashed_notebooks.push(PackagedTrashEntry {
            deleted_at_ms,
            document: crate::load_document_bytes(item_bytes)?,
        });
    }

    if reader.remaining() != 0 {
        return Err(FormatError::BinaryCorrupt(
            "unexpected trailing bytes in package payload".to_string(),
        ));
    }

    Ok(WorkspacePackage {
        exported_at_ms,
        source_name,
        prefs,
        notebooks,
        trashed_notebooks,
    })
}

fn write_u32(out: &mut Vec<u8>, len: usize) -> Result<(), FormatError> {
    let n = u32::try_from(len).map_err(|_| FormatError::PayloadTooLarge {
        limit_bytes: u32::MAX as usize,
    })?;
    out.extend_from_slice(&n.to_le_bytes());
    Ok(())
}

fn write_len_bytes(out: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), FormatError> {
    if bytes.len() > limit {
        return Err(FormatError::PayloadTooLarge { limit_bytes: limit });
    }
    write_u32(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn decompress_bounded(compressed: &[u8]) -> Result<Vec<u8>, FormatError> {
    let cap = compressed
        .len()
        .saturating_mul(MAX_DECOMPRESSION_RATIO)
        .clamp(MIN_DECOMPRESSED_BYTES_CAP, MAX_DECOMPRESSED_BYTES_CAP);
    let decoder = ZlibDecoder::new(compressed);
    let mut limited = decoder.take((cap + 1) as u64);
    let mut raw = Vec::new();
    limited
        .read_to_end(&mut raw)
        .map_err(|e| FormatError::Compression(e.to_string()))?;
    if raw.len() > cap {
        return Err(FormatError::PayloadTooLarge { limit_bytes: cap });
    }
    Ok(raw)
}

struct SliceReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceReader<'a> {
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| FormatError::BinaryCorrupt("integer overflow in reader".to_string()))?;
        let slice = self.data.get(self.pos..end).ok_or_else(|| {
            FormatError::BinaryCorrupt("unexpected EOF in package payload".to_string())
        })?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u32(&mut self) -> Result<u32, FormatError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i64(&mut self) -> Result<i64, FormatError> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_count(&mut self, max: usize, label: &str) -> Result<usize, FormatError> {
        let n = usize::try_from(self.read_u32()?)
            .map_err(|_| FormatError::PayloadTooLarge { limit_bytes: max })?;
        if n > max {
            return Err(FormatError::BinaryCorrupt(format!(
                "package {label} count {n} exceeds limit {max}"
            )));
        }
        Ok(n)
    }

    fn read_len_bytes(&mut self, max: usize) -> Result<&'a [u8], FormatError> {
        let len = self.read_count(max, "entry byte length")?;
        self.take(len)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_doc::NotebookCover;

    #[test]
    fn package_round_trip_preserves_notebooks_trash_and_prefs() {
        let doc1 = Document {
            id: "nb-one".to_string(),
            title: "Math".to_string(),
            cover: NotebookCover::ClassicNavy,
            ..Default::default()
        };

        let doc2 = Document {
            id: "nb-two".to_string(),
            title: "Old Physics".to_string(),
            ..Default::default()
        };

        let mut prefs = UserStudioConfig::default();
        prefs.library_folders.push("School".to_string());
        prefs.ui_scale = 1.25;

        let pkg = WorkspacePackage {
            exported_at_ms: 1_700_000_123_456,
            source_name: "Bruno".to_string(),
            prefs: prefs.clone(),
            notebooks: vec![doc1.clone()],
            trashed_notebooks: vec![PackagedTrashEntry {
                deleted_at_ms: 1_700_000_000_000,
                document: doc2.clone(),
            }],
        };

        let encoded = save_package_bytes(&pkg).unwrap();
        assert!(looks_like_package_bytes(&encoded));
        let decoded = load_package_bytes(&encoded).unwrap();
        assert_eq!(decoded.exported_at_ms, pkg.exported_at_ms);
        assert_eq!(decoded.source_name, "Bruno");
        assert_eq!(decoded.prefs.library_folders, vec!["School".to_string()]);
        assert_eq!(decoded.notebooks.len(), 1);
        assert_eq!(decoded.notebooks[0].id, "nb-one");
        assert_eq!(decoded.notebooks[0].title, "Math");
        assert_eq!(decoded.trashed_notebooks.len(), 1);
        assert_eq!(
            decoded.trashed_notebooks[0].deleted_at_ms,
            1_700_000_000_000
        );
        assert_eq!(decoded.trashed_notebooks[0].document.id, "nb-two");
    }

    #[test]
    fn package_decoder_rejects_corrupt_and_unsupported_inputs() {
        assert!(!looks_like_package_bytes(b"PCFT\x02\x00\x00\x00"));
        assert!(matches!(
            load_package_bytes(b"short"),
            Err(FormatError::BinaryCorrupt(_))
        ));
        assert!(matches!(
            load_package_bytes(b"BAD!\x01\x00\x00\x00"),
            Err(FormatError::InvalidFormat(_))
        ));
        assert!(matches!(
            load_package_bytes(b"PCPK\x99\x00\x00\x00"),
            Err(FormatError::UnsupportedVersion(0x99))
        ));
        assert!(matches!(
            load_package_bytes(b"NCPK\x99\x00\x00\x00"),
            Err(FormatError::UnsupportedVersion(0x99))
        ));
    }
}
