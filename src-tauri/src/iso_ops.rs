use crate::error::InstallerError;
use crate::image_ops;
use crate::squashfs_ops;
use crate::util::run_powershell;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LinuxKernelInfo {
    pub kernel_path: String,
    pub initrd_path: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IsoLayoutInfo {
    pub squashfs_compressed_mb: u64,
    pub squashfs_uncompressed_mb: u64,
    pub min_root_disk_gb: u32,
    pub suggested_root_disk_gb: u32,
    pub has_uefi_kernel: bool,
    /// True when the stock initrd appears to ship NTFS (or ntfs3) support.
    pub has_ntfs_in_initrd: bool,
}

pub fn get_iso_size_mb(iso_path: &str) -> Result<u64, InstallerError> {
    let metadata = std::fs::metadata(iso_path)
        .map_err(|e| InstallerError::Io(format!("ISO size read failed: {}", e)))?;
    Ok(metadata.len() / (1024 * 1024))
}

const KERNEL_SEARCH_PATHS: &[(&str, &str)] = &[
    ("live/vmlinuz", "live/initrd.img"),
    ("live/vmlinuz", "live/initrd"),
    ("casper/vmlinuz", "casper/initrd"),
    ("casper/vmlinuz", "casper/initrd.lz"),
    ("casper/vmlinuz", "casper/initrd.gz"),
    ("boot/vmlinuz", "boot/initrd.img"),
];

const SQUASHFS_SEARCH_PATHS: &[&str] = &[
    "live/filesystem.squashfs",
    "casper/filesystem.squashfs",
    "LiveOS/squashfs.img",
    "arch/x86_64/airootfs.sfs",
];

pub fn mount_iso(iso_path: &str) -> Result<String, InstallerError> {
    if !std::path::Path::new(iso_path).exists() {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_ISO_NOT_FOUND",
            &[("path", iso_path)],
        ));
    }

    if !iso_path.to_lowercase().ends_with(".iso") {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_NOT_ISO",
            &[],
        ));
    }

    validate_iso_file(iso_path)?;

    let _ = unmount_iso(iso_path);

    // Poll for the volume instead of a fixed sleep — usually ready in <500ms.
    let script = format!(
        r#"
        $path = '{}'
        try {{
            $img = Mount-DiskImage -ImagePath $path -PassThru -ErrorAction Stop
        }} catch {{
            throw ("MOUNT_FAIL:" + $_.Exception.Message)
        }}
        $letter = $null
        for ($i = 0; $i -lt 40; $i++) {{
            $vol = $img | Get-Volume -ErrorAction SilentlyContinue
            if ($vol -and $vol.DriveLetter) {{ $letter = $vol.DriveLetter; break }}
            Start-Sleep -Milliseconds 250
        }}
        if ($letter) {{ $letter }}
        else {{ throw "MOUNT_FAIL:ISO mounted but no drive letter assigned" }}
        "#,
        iso_path.replace('\'', "''")
    );

    let output = run_powershell(&script).map_err(|e| {
        let detail = e.to_string();
        let lower = detail.to_ascii_lowercase();
        let hint = if lower.contains("being used")
            || lower.contains("in use")
            || lower.contains("cannot access")
            || lower.contains("denied")
        {
            "ERR_ISO_MOUNT_LOCKED"
        } else {
            "ERR_ISO_MOUNT_FAILED"
        };
        InstallerError::coded(
            InstallerError::IsoExtraction,
            hint,
            &[("detail", detail.trim())],
        )
    })?;

    let letter = output.trim().to_string();
    if letter.len() != 1 || !letter.chars().next().unwrap_or(' ').is_ascii_alphabetic() {
        return Err(InstallerError::coded(
            InstallerError::IsoExtraction,
            "ERR_ISO_MOUNT_FAILED",
            &[("detail", &format!("invalid drive letter '{letter}'"))],
        ));
    }
    Ok(letter)
}

/// Reject archives misnamed as .iso (WinRAR/7-Zip) and non-ISO9660 payloads.
fn validate_iso_file(iso_path: &str) -> Result<(), InstallerError> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(iso_path)
        .map_err(|e| InstallerError::Io(format!("ISO open failed: {e}")))?;
    let mut head = [0u8; 16];
    let n = f.read(&mut head).unwrap_or(0);
    if (n >= 7 && &head[0..7] == b"Rar!\x1a\x07\x00")
        || (n >= 8 && &head[0..8] == b"Rar!\x1a\x07\x01\x00")
        || (n >= 4 && &head[0..4] == b"7z\xbc\xaf")
        || (n >= 4 && &head[0..4] == b"PK\x03\x04")
    {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_ISO_IS_ARCHIVE",
            &[],
        ));
    }

    // ISO 9660 Primary Volume Descriptor: "CD001" at sector 16 (offset 0x8001).
    const CD001_OFFSETS: &[u64] = &[0x8001, 0x8801, 0x9001];
    let mut found = false;
    for &off in CD001_OFFSETS {
        let mut magic = [0u8; 5];
        if f.seek(SeekFrom::Start(off)).is_err() {
            continue;
        }
        if f.read_exact(&mut magic).is_ok() && &magic == b"CD001" {
            found = true;
            break;
        }
    }
    if !found {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_ISO_INVALID",
            &[],
        ));
    }
    Ok(())
}

pub fn unmount_iso(iso_path: &str) -> Result<(), InstallerError> {
    let script = format!(
        "Dismount-DiskImage -ImagePath '{}' -ErrorAction SilentlyContinue",
        iso_path.replace('\'', "''")
    );
    let _ = run_powershell(&script);
    Ok(())
}

pub fn find_linux_kernel(iso_drive_letter: &str) -> Result<LinuxKernelInfo, InstallerError> {
    let root = format!("{}:\\", iso_drive_letter);
    let root_path = std::path::Path::new(&root);

    for (kernel_rel, initrd_rel) in KERNEL_SEARCH_PATHS {
        let kernel_path = root_path.join(kernel_rel.replace('/', "\\"));
        let initrd_path = root_path.join(initrd_rel.replace('/', "\\"));
        if kernel_path.exists() && initrd_path.exists() {
            return Ok(LinuxKernelInfo {
                kernel_path: kernel_rel.to_string(),
                initrd_path: initrd_rel.to_string(),
            });
        }
    }
    search_kernel_recursive(root_path)
}

pub fn find_squashfs_path(iso_drive_letter: &str) -> Result<String, InstallerError> {
    let root = format!("{}:\\", iso_drive_letter);
    let root_path = std::path::Path::new(&root);

    for candidate in SQUASHFS_SEARCH_PATHS {
        if root_path.join(candidate.replace('/', "\\")).exists() {
            return Ok((*candidate).to_string());
        }
    }
    search_squashfs_recursive(root_path).ok_or_else(|| {
        InstallerError::IsoExtraction(
            "filesystem.squashfs not found inside ISO".into(),
        )
    })
}

fn search_kernel_recursive(root: &std::path::Path) -> Result<LinuxKernelInfo, InstallerError> {
    let mut vmlinuz: Option<String> = None;
    let mut initrd: Option<String> = None;
    scan_dir(root, root, &mut vmlinuz, &mut initrd, 0);
    match (vmlinuz, initrd) {
        (Some(k), Some(i)) => Ok(LinuxKernelInfo {
            kernel_path: k,
            initrd_path: i,
        }),
        _ => Err(InstallerError::IsoExtraction(
            "Linux kernel (vmlinuz/initrd) not found inside ISO".into(),
        )),
    }
}

fn search_squashfs_recursive(root: &std::path::Path) -> Option<String> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, depth: u32) -> Option<String> {
        if depth > 5 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if path.is_file()
                && (name.ends_with(".squashfs") || name == "airootfs.sfs" || name == "squashfs.img")
            {
                if let Ok(rel) = path.strip_prefix(root) {
                    return Some(rel.to_string_lossy().replace('\\', "/"));
                }
            } else if path.is_dir() {
                if let Some(hit) = walk(&path, root, depth + 1) {
                    return Some(hit);
                }
            }
        }
        None
    }
    walk(root, root, 0)
}

fn scan_dir(
    dir: &std::path::Path,
    root: &std::path::Path,
    vmlinuz: &mut Option<String>,
    initrd: &mut Option<String>,
    depth: u32,
) {
    if depth > 5 || (vmlinuz.is_some() && initrd.is_some()) {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if path.is_file() {
            if name.starts_with("vmlinuz") && vmlinuz.is_none() {
                if let Ok(rel) = path.strip_prefix(root) {
                    *vmlinuz = Some(rel.to_string_lossy().replace('\\', "/"));
                }
            } else if (name.starts_with("initrd") || name.starts_with("initramfs"))
                && initrd.is_none()
            {
                if let Ok(rel) = path.strip_prefix(root) {
                    *initrd = Some(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        } else if path.is_dir() {
            scan_dir(&path, root, vmlinuz, initrd, depth + 1);
        }
    }
}

pub fn copy_iso_file(iso_drive_letter: &str, rel_path: &str, dest: &std::path::Path) -> Result<(), InstallerError> {
    let src = format!(
        "{}:\\{}",
        iso_drive_letter,
        rel_path.replace('/', "\\")
    );
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| InstallerError::Io(format!("dest parent create failed: {}", e)))?;
    }
    std::fs::copy(&src, dest)
        .map_err(|e| InstallerError::Io(format!("file copy failed ({} -> {}): {}", src, dest.display(), e)))?;
    Ok(())
}

fn squashfs_host_path(iso_drive_letter: &str, squashfs_rel: &str) -> String {
    format!(
        "{}:\\{}",
        iso_drive_letter.trim_end_matches(':'),
        squashfs_rel.replace('/', "\\")
    )
}

fn initrd_host_path(iso_drive_letter: &str, initrd_rel: &str) -> String {
    format!(
        "{}:\\{}",
        iso_drive_letter.trim_end_matches(':'),
        initrd_rel.replace('/', "\\")
    )
}

/// Heuristic scan of the ISO's stock initrd for NTFS kernel modules.
pub fn probe_initrd_has_ntfs(initrd_path: &Path) -> Result<bool, InstallerError> {
    let raw = std::fs::read(initrd_path)
        .map_err(|e| InstallerError::Io(format!("initrd read failed: {}", e)))?;
    Ok(initrd_bytes_have_ntfs(&raw))
}

fn is_gzip(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b
}

fn is_zstd(data: &[u8]) -> bool {
    data.len() >= 4 && data[0] == 0x28 && data[1] == 0xb5 && data[2] == 0x2f && data[3] == 0xfd
}

fn try_gzip_decompress(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = GzDecoder::new(data);
    let mut out = Vec::new();
    if dec.read_to_end(&mut out).is_ok() && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}

fn try_zstd_decompress(data: &[u8]) -> Option<Vec<u8>> {
    zstd::stream::decode_all(data).ok().filter(|o| !o.is_empty())
}

/// Skip leading uncompressed newc cpio archives (microcode / early initramfs).
fn skip_leading_cpio(data: &[u8]) -> &[u8] {
    let mut offset = 0usize;
    while data.len() >= offset + 110 && &data[offset..offset + 6] == b"070701" {
        let hdr = &data[offset..offset + 110];
        let namesize = usize::from_str_radix(
            std::str::from_utf8(&hdr[94..102]).unwrap_or(""),
            16,
        )
        .unwrap_or(0);
        let filesize = usize::from_str_radix(
            std::str::from_utf8(&hdr[54..62]).unwrap_or(""),
            16,
        )
        .unwrap_or(0);
        if namesize == 0 {
            break;
        }
        let name_off = offset + 110;
        let name_end = name_off.saturating_add(namesize);
        if name_end > data.len() {
            break;
        }
        let name = &data[name_off..name_end.saturating_sub(1)]; // strip NUL
        let name_pad = (4 - (name_end % 4)) % 4;
        let data_off = name_end + name_pad;
        let data_end = data_off.saturating_add(filesize);
        if data_end > data.len() {
            break;
        }
        let data_pad = (4 - (data_end % 4)) % 4;
        offset = data_end + data_pad;
        if name == b"TRAILER!!!" {
            // Keep skipping if another uncompressed cpio follows.
            continue;
        }
    }
    &data[offset.min(data.len())..]
}

fn collect_initrd_scan_buffers(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut bufs: Vec<Vec<u8>> = Vec::new();
    let push_unique = |bufs: &mut Vec<Vec<u8>>, b: Vec<u8>| {
        if !bufs.iter().any(|x| x.len() == b.len() && x == &b) {
            bufs.push(b);
        }
    };

    push_unique(&mut bufs, raw.to_vec());

    // After early uncompressed cpio (+ NUL padding), locate gzip/zstd payload.
    // Pardus/Debian live initrds often place zstd tens of MB into the file.
    let mut offsets: Vec<usize> = Vec::new();
    let after = skip_leading_cpio(raw);
    let after_off = raw.len().saturating_sub(after.len());
    let mut pad = after_off;
    while pad < raw.len() && raw[pad] == 0 {
        pad += 1;
    }
    if pad < raw.len() {
        offsets.push(pad);
    }
    for i in 0..raw.len().saturating_sub(4) {
        if is_zstd(&raw[i..]) || is_gzip(&raw[i..]) {
            offsets.push(i);
            break; // first compressor frame is the main initramfs
        }
    }

    for off in offsets {
        let chunk = &raw[off..];
        if is_gzip(chunk) {
            if let Some(d) = try_gzip_decompress(chunk) {
                push_unique(&mut bufs, d);
            }
        }
        if is_zstd(chunk) {
            if let Some(d) = try_zstd_decompress(chunk) {
                push_unique(&mut bufs, d);
            }
        }
    }

    bufs
}

fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn initrd_payload_has_ntfs(payload: &[u8]) -> bool {
    const NEEDLES: &[&[u8]] = &[
        b"ntfs3.ko",
        b"ntfs.ko",
        b"ntfs3.ko.xz",
        b"ntfs3.ko.zst",
        b"ntfs.ko.xz",
        b"kernel/fs/ntfs",
        b"kernel/fs/ntfs3",
        b"modules/ntfs",
        b"modules/ntfs3",
        b"bin/ntfs-3g",
        b"sbin/ntfs-3g",
        b"ntfs-3g",
    ];
    NEEDLES.iter().any(|needle| bytes_contain(payload, needle))
}

fn initrd_bytes_have_ntfs(raw: &[u8]) -> bool {
    collect_initrd_scan_buffers(raw)
        .iter()
        .any(|buf| initrd_payload_has_ntfs(buf))
}

#[cfg(test)]
mod ntfs_probe_tests {
    use super::*;

    #[test]
    fn detects_ntfs3_in_plain_cpio_bytes() {
        let mut buf = b"070701....kernel/fs/ntfs3/ntfs3.ko\0".to_vec();
        assert!(initrd_payload_has_ntfs(&buf));
        buf = b"no filesystem here".to_vec();
        assert!(!initrd_payload_has_ntfs(&buf));
    }

    #[test]
    fn probe_testdata_initrd_if_present() {
        let path = std::path::Path::new("testdata-initrd.img");
        if !path.exists() {
            eprintln!("skip: testdata-initrd.img missing");
            return;
        }
        let raw = std::fs::read(path).expect("read testdata");
        eprintln!("size={}", raw.len());
        eprintln!("head={:?}", String::from_utf8_lossy(&raw[..6.min(raw.len())]));

        let rest = skip_leading_cpio(&raw);
        eprintln!(
            "after_cpio_skip offset={} remaining={}",
            raw.len() - rest.len(),
            rest.len()
        );
        if rest.len() >= 4 {
            eprintln!(
                "rest_magic={:02x} {:02x} {:02x} {:02x}",
                rest[0], rest[1], rest[2], rest[3]
            );
        }

        let mut zstd_off = None;
        for i in 0..raw.len().saturating_sub(4) {
            if is_zstd(&raw[i..]) {
                zstd_off = Some(i);
                break;
            }
        }
        eprintln!("first_zstd_off={:?}", zstd_off);

        let hit = initrd_bytes_have_ntfs(&raw);
        eprintln!("has_ntfs={}", hit);
        assert!(hit, "expected NTFS modules in Pardus live initrd");
    }
}

/// Mount the ISO briefly and derive sizing guidance from its squashfs payload.
pub fn probe_iso_layout(iso_path: &str) -> Result<IsoLayoutInfo, InstallerError> {
    let iso_drive = mount_iso(iso_path)?;
    let result = (|| {
        let kernel = find_linux_kernel(&iso_drive)?;
        let has_uefi_kernel = true;
        let initrd_path = initrd_host_path(&iso_drive, &kernel.initrd_path);
        let has_ntfs_in_initrd = probe_initrd_has_ntfs(Path::new(&initrd_path))?;
        let squashfs_rel = find_squashfs_path(&iso_drive)?;
        let squashfs_path = squashfs_host_path(&iso_drive, &squashfs_rel);
        // Some desktop live images (e.g. Pardus GNOME) omit ntfs3 from the
        // stock initrd even though the squashfs ships the module. Treat either
        // as installable — the installer injects the module into our overlay.
        let has_ntfs_in_squashfs = squashfs_ops::squashfs_has_ntfs3(Path::new(&squashfs_path))?;
        let has_ntfs_in_initrd = has_ntfs_in_initrd || has_ntfs_in_squashfs;
        let compressed = std::fs::metadata(&squashfs_path)
            .map_err(|e| InstallerError::Io(format!("squashfs stat failed: {}", e)))?
            .len();
        let uncompressed = squashfs_ops::read_uncompressed_size(Path::new(&squashfs_path))?;
        let min_gb = image_ops::min_root_disk_gb(uncompressed);
        let suggested_gb = image_ops::suggested_root_disk_gb(min_gb);
        Ok(IsoLayoutInfo {
            squashfs_compressed_mb: compressed / (1024 * 1024),
            squashfs_uncompressed_mb: uncompressed / (1024 * 1024),
            min_root_disk_gb: min_gb,
            suggested_root_disk_gb: suggested_gb,
            has_uefi_kernel,
            has_ntfs_in_initrd,
        })
    })();
    let _ = unmount_iso(iso_path);
    result
}
