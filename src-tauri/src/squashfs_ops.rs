use crate::error::InstallerError;
use backhand::{FilesystemReader, InnerNode};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

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

    let compressed = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(estimate_uncompressed_from_compressed(compressed))
}

pub fn estimate_uncompressed_from_compressed(compressed_bytes: u64) -> u64 {
    compressed_bytes.saturating_mul(38) / 10
}

#[derive(Debug, Clone)]
pub struct NtfsModuleBlob {
    /// Path inside the overlay/initrd, e.g. `lib/modules/.../kernel/fs/ntfs3/ntfs3.ko`
    pub cpio_path: String,
    pub data: Vec<u8>,
}

fn node_full_path(node: &backhand::Node<backhand::SquashfsFileReader>) -> String {
    node.fullpath
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('\\', "/")
}

pub fn squashfs_has_ntfs3(path: &Path) -> Result<bool, InstallerError> {
    Ok(find_ntfs3_in_squashfs(path)?.is_some())
}

/// Extract `ntfs3.ko` (decompressing `.xz` / `.zst` if needed) for overlay injection.
pub fn extract_ntfs3_module(path: &Path) -> Result<Option<NtfsModuleBlob>, InstallerError> {
    let Some((rel, raw)) = find_ntfs3_in_squashfs(path)? else {
        return Ok(None);
    };

    let (cpio_path, data) = if rel.ends_with(".ko.xz") {
        let dec = decompress_xz(&raw).map_err(|e| {
            InstallerError::IsoExtraction(format!("ntfs3.ko.xz decompress failed: {e}"))
        })?;
        (rel.trim_end_matches(".xz").to_string(), dec)
    } else if rel.ends_with(".ko.zst") {
        let dec = zstd::stream::decode_all(raw.as_slice()).map_err(|e| {
            InstallerError::IsoExtraction(format!("ntfs3.ko.zst decompress failed: {e}"))
        })?;
        (rel.trim_end_matches(".zst").to_string(), dec)
    } else if rel.ends_with(".ko.gz") {
        use flate2::read::GzDecoder;
        let mut dec = GzDecoder::new(raw.as_slice());
        let mut out = Vec::new();
        dec.read_to_end(&mut out).map_err(|e| {
            InstallerError::IsoExtraction(format!("ntfs3.ko.gz decompress failed: {e}"))
        })?;
        (rel.trim_end_matches(".gz").to_string(), out)
    } else {
        (rel, raw)
    };

    Ok(Some(NtfsModuleBlob { cpio_path, data }))
}

fn decompress_xz(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = liblzma::read::XzDecoder::new(raw);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn find_ntfs3_in_squashfs(path: &Path) -> Result<Option<(String, Vec<u8>)>, InstallerError> {
    let file = std::fs::File::open(path)
        .map_err(|e| InstallerError::IsoExtraction(format!("squashfs open failed: {}", e)))?;
    let reader = BufReader::new(file);
    let fs = FilesystemReader::from_reader(reader).map_err(|e| {
        InstallerError::IsoExtraction(format!("squashfs parse failed: {}", e))
    })?;

    let mut preferred: Option<(String, Vec<u8>)> = None;
    for node in fs.files() {
        let InnerNode::File(file) = &node.inner else {
            continue;
        };
        let full = node_full_path(node);
        let lower = full.to_ascii_lowercase();
        let is_ntfs3 = lower.contains("/kernel/fs/ntfs3/")
            && (lower.ends_with("ntfs3.ko")
                || lower.ends_with("ntfs3.ko.xz")
                || lower.ends_with("ntfs3.ko.zst")
                || lower.ends_with("ntfs3.ko.gz"));
        if !is_ntfs3 {
            continue;
        }

        let mut reader = fs.file(file).reader();
        let mut data = Vec::new();
        reader.read_to_end(&mut data).map_err(|e| {
            InstallerError::IsoExtraction(format!("squashfs read {full} failed: {e}"))
        })?;

        if lower.ends_with("ntfs3.ko") {
            return Ok(Some((full, data)));
        }
        if preferred.is_none() {
            preferred = Some((full, data));
        }
    }
    Ok(preferred)
}

pub fn module_parent_dirs(cpio_path: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut cur = PathBuf::new();
    let parent = Path::new(cpio_path).parent().unwrap_or_else(|| Path::new(""));
    for comp in parent.components() {
        cur.push(comp);
        dirs.push(cur.to_string_lossy().replace('\\', "/"));
    }
    dirs
}

