//! CRI AFS (AFS Packed File System) archive reader.
//!
//! God Hand stores all 1563 game assets inside `_DATA.AFS` (1.94 GB).
//! Layout (verified against the shipped artifact):
//!
//! ```text
//! 0x00  magic        b"AFS\0"
//! 0x04  file_count   u32 LE
//! 0x08  flags        u16 LE   (0x3800 in this build)
//! 0x0A  version      u16 LE
//! 0x0C  align        u32 LE   (33152 = 0x8180, sector alignment)
//! 0x10  toc[file_count] { offset u32 LE, size u32 LE }
//! ...   optional embedded name table (absent here - names live in GODHAND.FST)
//! ...   file payloads, each aligned to `align`
//! ```
//!
//! The embedded name table is absent in this build, so names are resolved
//! externally from `GODHAND.FST` and attached by index.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Magic bytes at the start of every AFS archive.
pub const AFS_MAGIC: &[u8; 4] = b"AFS\0";

/// Header size in bytes (magic + count + flags + version + align).
pub const AFS_HEADER_SIZE: u64 = 0x10;

/// One table-of-contents entry describing a stored file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfsEntry {
    /// Byte offset of the payload inside the archive.
    pub offset: u32,
    /// Payload size in bytes.
    pub size: u32,
}

/// A parsed AFS archive header plus its table of contents.
#[derive(Debug, Clone)]
pub struct Afs {
    /// Raw header fields, preserved for provenance.
    pub file_count: u32,
    pub flags: u16,
    pub version: u16,
    pub align: u32,
    /// Table of contents, ordered by archive index.
    pub entries: Vec<AfsEntry>,
    /// Byte offset where the TOC ends and payload data may begin.
    pub data_start: u64,
}

impl Afs {
    /// Parse the header and table of contents from an in-memory image.
    ///
    /// This only reads metadata; payloads stay on disk until `extract` is called.
    pub fn parse(bytes: &[u8]) -> io::Result<Afs> {
        if bytes.len() < AFS_HEADER_SIZE as usize {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "buffer smaller than AFS header",
            ));
        }
        if &bytes[0..4] != AFS_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not an AFS archive (bad magic)",
            ));
        }

        let file_count = read_u32_le(bytes, 0x04);
        let flags = read_u16_le(bytes, 0x08);
        let version = read_u16_le(bytes, 0x0A);
        let align = read_u32_le(bytes, 0x0C);

        let toc_bytes = AFS_HEADER_SIZE + (file_count as u64) * 8;
        if bytes.len() < toc_bytes as usize {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "buffer smaller than AFS table of contents",
            ));
        }

        let mut entries = Vec::with_capacity(file_count as usize);
        for i in 0..file_count as usize {
            let base = AFS_HEADER_SIZE as usize + i * 8;
            entries.push(AfsEntry {
                offset: read_u32_le(bytes, base),
                size: read_u32_le(bytes, base + 4),
            });
        }

        Ok(Afs {
            file_count,
            flags,
            version,
            align,
            entries,
            data_start: toc_bytes,
        })
    }

    /// Open an AFS archive from disk and parse its header and TOC.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Afs> {
        let mut file = File::open(path)?;
        // The TOC for 1563 entries is ~12.5 KB; read generously to cover it.
        let probe_len = (AFS_HEADER_SIZE + 200_000) as usize;
        let mut buf = vec![0u8; probe_len];
        let read = file.read(&mut buf)?;
        buf.truncate(read);
        Afs::parse(&buf)
    }

    /// Number of files described by the table of contents.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the archive describes zero files.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Fetch one entry by archive index.
    pub fn entry(&self, index: usize) -> Option<&AfsEntry> {
        self.entries.get(index)
    }

    /// Read one payload into memory by archive index.
    pub fn read_entry<P: AsRef<Path>>(&self, path: P, index: usize) -> io::Result<Vec<u8>> {
        let entry = self.entries.get(index).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("AFS index {index} out of range (count {})", self.len()),
            )
        })?;
        self.read_at(path, entry)
    }

    /// Read a payload described by an explicit TOC entry.
    pub fn read_at<P: AsRef<Path>>(&self, path: P, entry: &AfsEntry) -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(entry.offset as u64))?;
        let mut buf = vec![0u8; entry.size as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Iterate entries with their archive indices.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &AfsEntry)> {
        self.entries.iter().enumerate()
    }
}

/// Read a little-endian `u16` at `offset`, returning 0 when out of bounds.
fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    if offset + 2 > bytes.len() {
        return 0;
    }
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// Read a little-endian `u32` at `offset`, returning 0 when out of bounds.
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    if offset + 4 > bytes.len() {
        return 0;
    }
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic archive: 2 entries, payloads at aligned offsets.
    fn fixture() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(AFS_MAGIC);
        v.extend_from_slice(&2u32.to_le_bytes()); // count
        v.extend_from_slice(&0x3800u16.to_le_bytes()); // flags
        v.extend_from_slice(&0u16.to_le_bytes()); // version
        v.extend_from_slice(&33152u32.to_le_bytes()); // align
        // TOC
        v.extend_from_slice(&0x8000u32.to_le_bytes());
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&0x9000u32.to_le_bytes());
        v.extend_from_slice(&32u32.to_le_bytes());
        // pad to first payload
        v.resize(0x8000, 0);
        v.extend_from_slice(b"FIRSTPAYLOAD!!!!");
        v.resize(0x9000, 0);
        v.extend_from_slice(b"SECONDPAYLOAD_SECONDPAYLOAD_SECOND");
        v
    }

    #[test]
    fn parses_header_and_toc() {
        let afs = Afs::parse(&fixture()).expect("parse");
        assert_eq!(afs.file_count, 2);
        assert_eq!(afs.flags, 0x3800);
        assert_eq!(afs.align, 33152);
        assert_eq!(afs.len(), 2);
        assert_eq!(afs.entries[0].offset, 0x8000);
        assert_eq!(afs.entries[0].size, 16);
        assert_eq!(afs.entries[1].offset, 0x9000);
        assert_eq!(afs.entries[1].size, 32);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = fixture();
        bad[0] = b'X';
        assert!(Afs::parse(&bad).is_err());
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(Afs::parse(&[0u8; 8]).is_err());
    }

    #[test]
    fn rejects_truncated_toc() {
        let mut short = fixture();
        short.truncate(0x18);
        assert!(Afs::parse(&short).is_err());
    }
}
