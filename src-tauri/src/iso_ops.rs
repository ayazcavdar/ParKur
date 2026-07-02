use crate::error::InstallerError;
use crate::util::run_powershell;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LinuxKernelInfo {
    pub kernel_path: String,
    pub initrd_path: String,
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
        return Err(InstallerError::InvalidInput(format!(
            "ISO file not found: {}",
            iso_path
        )));
    }

    if !iso_path.to_lowercase().ends_with(".iso") {
        return Err(InstallerError::InvalidInput(
            "Selected file is not an ISO".into(),
        ));
    }

    let _ = unmount_iso(iso_path);

    // Poll for the volume instead of a fixed sleep — usually ready in <500ms.
    let script = format!(
        r#"
        $img = Mount-DiskImage -ImagePath '{}' -PassThru -ErrorAction Stop
        $letter = $null
        for ($i = 0; $i -lt 40; $i++) {{
            $vol = $img | Get-Volume -ErrorAction SilentlyContinue
            if ($vol -and $vol.DriveLetter) {{ $letter = $vol.DriveLetter; break }}
            Start-Sleep -Milliseconds 250
        }}
        if ($letter) {{ $letter }}
        else {{ throw "ISO mounted but no drive letter assigned" }}
        "#,
        iso_path.replace('\'', "''")
    );

    let output = run_powershell(&script)
        .map_err(|e| InstallerError::IsoExtraction(format!("ISO mount failed: {}", e)))?;

    let letter = output.trim().to_string();
    if letter.len() != 1 || !letter.chars().next().unwrap_or(' ').is_ascii_alphabetic() {
        return Err(InstallerError::IsoExtraction(format!(
            "Invalid ISO drive letter: '{}'",
            letter
        )));
    }
    Ok(letter)
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
