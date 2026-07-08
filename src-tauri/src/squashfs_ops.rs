use crate::error::InstallerError;
use backhand::{FilesystemReader, InnerNode};
use std::io::BufReader;
use std::path::Path;

/// Sum uncompressed file payload sizes inside a squashfs image (same figure
/// `unsquashfs -s` reports as "Filesystem size").
pub fn read_uncompressed_size(path: &Path) -> Result<u64, InstallerError> {
    let file = std::fs::File::open(path)
        .map_err(|e| InstallerError::IsoExtraction(format!("squashfs open failed: {}", e)))?;
    let reader = BufReader::new(file);
    let fs = FilesystemReader::from_reader(reader).map_err(|e| {
        InstallerError::IsoExtraction(format!("squashfs parse failed: {}", e))
    })?;

    let total: u64 = fs
        .files()
        .filter_map(|node| match &node.inner {
            InnerNode::File(file) => Some(file.file_len() as u64),
            _ => None,
        })
        .sum();

    if total > 0 {
        return Ok(total);
    }

    // Parser succeeded but found no files — fall back to a conservative ratio.
    let compressed = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(estimate_uncompressed_from_compressed(compressed))
}

/// Conservative fallback when squashfs metadata cannot be parsed.
pub fn estimate_uncompressed_from_compressed(compressed_bytes: u64) -> u64 {
    // Debian/Pardus desktop live images typically expand ~3.2–3.8×.
    compressed_bytes.saturating_mul(38) / 10
}
