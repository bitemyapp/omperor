//! A session's transcript heights, kept beside its journal as
//! `<id>.heights` so the next open lays every unchanged row out at the
//! height it had, and the scroll bar has its size and place from the first
//! frame.
//!
//! One small binary file per session rather than a table in the app's
//! database: the history worker that reads the journal reads it in the same
//! breath, with no lock shared with anything; it is written whole and moved
//! into place, so a reader never sees half of one; and it goes where the
//! session goes. Little-endian: magic, format, the width, type metrics and
//! renderer it was measured at, the content's total height, then each
//! piece's height by its content key and each Markdown row's height by its
//! fingerprint. Any mismatch in the header and the file is ignored: the rows
//! fall back to estimates.
use std::{
    io,
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 4] = b"OMPH";
const FORMAT: u32 = 1;
/// Bumped when a renderer change moves heights for the same text, width and
/// metrics: every saved file is then ignored once and written again.
pub(crate) const RENDERER: u32 = 1;
/// A file bigger than this is not one this module wrote.
const MAX_ENTRIES: u32 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct SavedHeights {
    /// The column width, as `f64::to_bits`.
    pub(crate) width: u64,
    /// `Styles::metrics_key`: the chat style and text sizes.
    pub(crate) metrics: u64,
    /// The pieces' heights added up.
    pub(crate) total: f64,
    /// Each piece's height by its content key.
    pub(crate) pieces: Vec<(u64, f64)>,
    /// Each Markdown row's pieces' heights added up, by its fingerprint:
    /// the row's height before its text is cut into pieces.
    pub(crate) rows: Vec<(u64, f64)>,
}

/// Where a session's heights live: beside its journal.
pub(crate) fn path_for(journal: &Path) -> PathBuf {
    journal.with_extension("heights")
}

pub(crate) fn encode(saved: &SavedHeights) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + 16 * (saved.pieces.len() + saved.rows.len()));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&RENDERER.to_le_bytes());
    out.extend_from_slice(&saved.width.to_le_bytes());
    out.extend_from_slice(&saved.metrics.to_le_bytes());
    out.extend_from_slice(&saved.total.to_le_bytes());
    for list in [&saved.pieces, &saved.rows] {
        out.extend_from_slice(&(list.len() as u32).to_le_bytes());
        for (key, height) in list.iter() {
            out.extend_from_slice(&key.to_le_bytes());
            out.extend_from_slice(&height.to_le_bytes());
        }
    }
    out
}

pub(crate) fn decode(bytes: &[u8]) -> Option<SavedHeights> {
    let mut at = 0;
    let mut take = |length: usize| -> Option<&[u8]> {
        let slice = bytes.get(at..at + length)?;
        at += length;
        Some(slice)
    };
    if take(4)? != MAGIC {
        return None;
    }
    let u32_at = |slice: &[u8]| u32::from_le_bytes(slice.try_into().expect("four bytes"));
    let u64_at = |slice: &[u8]| u64::from_le_bytes(slice.try_into().expect("eight bytes"));
    if u32_at(take(4)?) != FORMAT || u32_at(take(4)?) != RENDERER {
        return None;
    }
    let width = u64_at(take(8)?);
    let metrics = u64_at(take(8)?);
    let total = f64::from_bits(u64_at(take(8)?));
    let mut lists = [Vec::new(), Vec::new()];
    for list in &mut lists {
        let count = u32_at(take(4)?);
        if count > MAX_ENTRIES {
            return None;
        }
        list.reserve(count as usize);
        for _ in 0..count {
            let key = u64_at(take(8)?);
            let height = f64::from_bits(u64_at(take(8)?));
            if !height.is_finite() || height < 0.0 {
                return None;
            }
            list.push((key, height));
        }
    }
    let [pieces, rows] = lists;
    total.is_finite().then_some(SavedHeights { width, metrics, total, pieces, rows })
}

/// Off the main thread: the session's saved heights, if any are readable.
pub(crate) fn read(journal: &Path) -> Option<SavedHeights> {
    let _span = crate::perf::span("gui.saved_heights.read");
    decode(&std::fs::read(path_for(journal)).ok()?)
}

/// Off the main thread: writes the heights whole, then moves them into
/// place.
pub(crate) fn write(journal: &Path, saved: &SavedHeights) -> io::Result<()> {
    let _span = crate::perf::span("gui.saved_heights.write");
    let path = path_for(journal);
    let temporary = path.with_extension("heights.tmp");
    std::fs::write(&temporary, encode(saved))?;
    std::fs::rename(&temporary, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SavedHeights {
        SavedHeights {
            width: 640.0_f64.to_bits(),
            metrics: 0xfeed,
            total: 1234.5,
            pieces: vec![(1, 20.0), (2, 380.5)],
            rows: vec![(9, 400.5)],
        }
    }

    #[test]
    fn heights_survive_a_round_trip() {
        assert_eq!(decode(&encode(&sample())), Some(sample()));
    }

    #[test]
    fn a_short_or_foreign_file_is_ignored() {
        let bytes = encode(&sample());
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        let mut other = bytes.clone();
        other[8] ^= 1; // another renderer
        assert_eq!(decode(&other), None);
        assert_eq!(decode(b"not heights"), None);
    }

    #[test]
    fn written_beside_the_journal() {
        let directory = std::env::temp_dir().join(format!("omperor-saved-heights-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let journal = directory.join("01SESSION.oms");
        write(&journal, &sample()).unwrap();
        assert!(directory.join("01SESSION.heights").is_file());
        assert_eq!(read(&journal), Some(sample()));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
