//! GODHAND.FST manifest reader.
//!
//! The FST is the disc's master index: it names every asset inside
//! `_DATA.AFS`, which itself carries no embedded name table in this build.
//! Verified layout:
//!
//! ```text
//! 0x00  magic  b"FST\0"
//! 0x04  version f32 LE   (1.0)
//! 0x08  count   u32 LE   (16 - directory group count)
//! 0x0C  reserved f32 LE
//! 0x10  NUL-terminated ASCII string table (directories, then file names)
//! ...   binary record table (per-entry offset/size/flags)
//! ```
//!
//! This module exposes the string table and a filtered view of the file
//! names, which is what the asset pipeline needs to label AFS indices.

use std::collections::HashMap;
use std::fmt;

/// Error type for FST parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FstError {
    /// Buffer too small for the declared header.
    Truncated { needed: usize, have: usize },
    /// Magic bytes were not `FST\0`.
    BadMagic([u8; 4]),
    /// String table was not NUL-terminated within the buffer.
    UnterminatedString,
}

impl fmt::Display for FstError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FstError::Truncated { needed, have } => {
                write!(f, "FST truncated: need {needed} bytes, have {have}")
            }
            FstError::BadMagic(m) => write!(f, "FST bad magic: {m:?}"),
            FstError::UnterminatedString => write!(f, "FST string table not NUL-terminated"),
        }
    }
}

impl std::error::Error for FstError {}

/// Result alias for FST operations.
pub type FstResult<T> = Result<T, FstError>;

/// Magic bytes at the start of the manifest.
pub const FST_MAGIC: &[u8; 4] = b"FST\0";

/// Size of the fixed FST header.
pub const FST_HEADER_SIZE: usize = 0x10;

/// A parsed GODHAND.FST manifest.
#[derive(Debug, Clone)]
pub struct Fst {
    /// Version float from the header (1.0 in this build).
    pub version: f32,
    /// Directory group count from the header (16 in this build).
    pub group_count: u32,
    /// Every string in the table, in file order.
    pub strings: Vec<String>,
    /// Byte offset where the string table ended.
    pub string_table_end: usize,
}

impl Fst {
    /// Parse an FST buffer.
    ///
    /// The string table is read until the first byte sequence that no longer
    /// decodes as printable ASCII, which is where the binary records begin.
    pub fn parse(bytes: &[u8]) -> FstResult<Fst> {
        if bytes.len() < FST_HEADER_SIZE {
            return Err(FstError::Truncated {
                needed: FST_HEADER_SIZE,
                have: bytes.len(),
            });
        }
        if &bytes[0..4] != FST_MAGIC {
            return Err(FstError::BadMagic([
                bytes[0], bytes[1], bytes[2], bytes[3],
            ]));
        }

        let version = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let group_count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);

        let mut strings = Vec::new();
        let mut cursor = FST_HEADER_SIZE;
        while cursor < bytes.len() {
            // Find the next NUL terminator.
            let start = cursor;
            let mut end = cursor;
            while end < bytes.len() && bytes[end] != 0 {
                end += 1;
            }
            if end >= bytes.len() {
                return Err(FstError::UnterminatedString);
            }
            let slice = &bytes[start..end];
            // Stop when the table transitions into binary records.
            if !is_manifest_string(slice) {
                break;
            }
            match std::str::from_utf8(slice) {
                Ok(s) => strings.push(s.to_string()),
                Err(_) => break,
            }
            cursor = end + 1;
        }

        Ok(Fst {
            version,
            group_count,
            strings,
            string_table_end: cursor,
        })
    }

    /// File names only: strings that carry a recognized asset extension.
    ///
    /// Order matches the FST table, which corresponds to AFS archive order.
    pub fn file_names(&self) -> Vec<&str> {
        self.strings
            .iter()
            .map(|s| s.as_str())
            .filter(|s| has_asset_extension(s))
            .collect()
    }

    /// Directory names only: strings without an extension.
    pub fn directories(&self) -> Vec<&str> {
        self.strings
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty() && !has_asset_extension(s))
            .collect()
    }

    /// Group file names by their extension, counting each group.
    pub fn extension_histogram(&self) -> HashMap<String, usize> {
        let mut map = HashMap::new();
        for name in self.file_names() {
            if let Some(dot) = name.rfind('.') {
                *map.entry(name[dot + 1..].to_string()).or_insert(0) += 1;
            }
        }
        map
    }

    /// All `.rel` dynamic module names, in table order.
    pub fn rel_modules(&self) -> Vec<&str> {
        self.file_names()
            .into_iter()
            .filter(|s| s.ends_with(".rel"))
            .collect()
    }

    /// Look up the name for an AFS archive index.
    ///
    /// Returns `None` when the index is past the named file list.
    pub fn name_for_index(&self, index: usize) -> Option<&str> {
        self.file_names().get(index).copied()
    }
}

/// Whether a string looks like a manifest asset name rather than binary noise.
fn is_manifest_string(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'%' | b'-' | b'*'))
}

/// Whether a name carries one of the extensions God Hand uses.
fn has_asset_extension(name: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".dat", ".afd", ".afs", ".idd", ".ses", ".rel", ".tm2", ".sfd", ".bin", ".t32", ".bmt",
        ".atb",
    ];
    EXTENSIONS.iter().any(|ext| name.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(FST_MAGIC);
        v.extend_from_slice(&1.0f32.to_le_bytes());
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&0.0f32.to_le_bytes());
        for s in ["bgm", "dll", "em", "ol", "om", "pl", "bgm00.afs", "em00.dat", "r000.rel"] {
            v.extend_from_slice(s.as_bytes());
            v.push(0);
        }
        // Binary record region follows.
        v.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0xFF, 0xFF]);
        v
    }

    #[test]
    fn parses_header_and_strings() {
        let fst = Fst::parse(&fixture()).expect("parse");
        assert_eq!(fst.version, 1.0);
        assert_eq!(fst.group_count, 16);
        assert_eq!(fst.strings.len(), 9);
        assert_eq!(fst.strings[0], "bgm");
        assert_eq!(fst.strings[6], "bgm00.afs");
    }

    #[test]
    fn separates_files_from_directories() {
        let fst = Fst::parse(&fixture()).expect("parse");
        assert_eq!(fst.file_names(), vec!["bgm00.afs", "em00.dat", "r000.rel"]);
        assert_eq!(fst.directories(), vec!["bgm", "dll", "em", "ol", "om", "pl"]);
    }

    #[test]
    fn histogram_counts_extensions() {
        let fst = Fst::parse(&fixture()).expect("parse");
        let hist = fst.extension_histogram();
        assert_eq!(hist.get("afs"), Some(&1));
        assert_eq!(hist.get("dat"), Some(&1));
        assert_eq!(hist.get("rel"), Some(&1));
    }

    #[test]
    fn rel_modules_filtered() {
        let fst = Fst::parse(&fixture()).expect("parse");
        assert_eq!(fst.rel_modules(), vec!["r000.rel"]);
    }

    #[test]
    fn name_for_index_matches_file_order() {
        let fst = Fst::parse(&fixture()).expect("parse");
        assert_eq!(fst.name_for_index(0), Some("bgm00.afs"));
        assert_eq!(fst.name_for_index(2), Some("r000.rel"));
        assert_eq!(fst.name_for_index(3), None);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = fixture();
        bad[0] = b'X';
        assert!(matches!(Fst::parse(&bad), Err(FstError::BadMagic(_))));
    }

    #[test]
    fn rejects_short_buffer() {
        assert!(matches!(
            Fst::parse(&[0u8; 8]),
            Err(FstError::Truncated { .. })
        ));
    }
}
