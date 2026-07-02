use crate::error::InstallerError;
use crate::util::{run_powershell, CREATE_NO_WINDOW};
use std::io::{Seek, SeekFrom, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const NEXTOS_HOST_DIR_NAME: &str = "NextOS";
pub const ROOT_DISK_FILENAME: &str = "root.disk";
pub const SQUASHFS_FILENAME: &str = "filesystem.squashfs";
pub const PROVISIONING_FILENAME: &str = "nextos.conf";
pub const MIN_ROOT_DISK_GB: u32 = 10;
pub const MAX_ROOT_DISK_GB: u32 = 100;
pub const SAFETY_HEADROOM_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub const HOST_BOOT_DIR_NAME: &str = "boot";
pub const HOST_KERNEL_FILENAME: &str = "vmlinuz";
pub const HOST_INITRD_FILENAME: &str = "initrd.img";
pub const HOST_OVERLAY_FILENAME: &str = "overlay.cpio.gz";
/// Created by the initramfs /init after a successful mkfs.ext4; its absence
/// tells the first boot that root.disk is still raw. Removed on (re)install.
pub const FORMAT_MARKER_FILENAME: &str = ".nextos-formatted";

pub struct HostImageLayout {
    #[allow(dead_code)]
    pub drive_letter: String,
    pub host_dir: PathBuf,
    pub boot_dir: PathBuf,
    pub root_disk: PathBuf,
    pub squashfs: PathBuf,
    pub config: PathBuf,
    pub host_kernel: PathBuf,
    pub host_initrd: PathBuf,
    pub host_overlay: PathBuf,
    pub root_disk_unix_path: String,
    pub squashfs_unix_path: String,
}

pub fn build_layout(drive_letter: &str) -> HostImageLayout {
    let letter = drive_letter.trim_end_matches(':').to_string();
    let base = PathBuf::from(format!("{}:\\{}", letter, NEXTOS_HOST_DIR_NAME));
    let boot = base.join(HOST_BOOT_DIR_NAME);
    HostImageLayout {
        drive_letter: letter,
        root_disk: base.join(ROOT_DISK_FILENAME),
        squashfs: base.join(SQUASHFS_FILENAME),
        config: base.join(PROVISIONING_FILENAME),
        host_kernel: boot.join(HOST_KERNEL_FILENAME),
        host_initrd: boot.join(HOST_INITRD_FILENAME),
        host_overlay: boot.join(HOST_OVERLAY_FILENAME),
        boot_dir: boot,
        host_dir: base,
        root_disk_unix_path: format!("/{}/{}", NEXTOS_HOST_DIR_NAME, ROOT_DISK_FILENAME),
        squashfs_unix_path: format!("/{}/{}", NEXTOS_HOST_DIR_NAME, SQUASHFS_FILENAME),
    }
}

pub fn ensure_host_dir(layout: &HostImageLayout) -> Result<(), InstallerError> {
    std::fs::create_dir_all(&layout.host_dir)
        .map_err(|e| InstallerError::Io(format!("host dir create failed: {}", e)))
}

pub fn get_free_bytes(drive_letter: &str) -> Result<u64, InstallerError> {
    let letter = drive_letter.trim_end_matches(':');
    let script = format!(
        r#"(Get-Volume -DriveLetter '{}').SizeRemaining"#,
        letter
    );
    let raw = run_powershell(&script)?;
    raw.trim().parse::<u64>().map_err(|_| {
        InstallerError::DiskOperation(format!("free space query failed: '{}'", raw.trim()))
    })
}

pub fn validate_capacity(
    drive_letter: &str,
    root_disk_bytes: u64,
    squashfs_bytes: u64,
) -> Result<(), InstallerError> {
    let free = get_free_bytes(drive_letter)?;
    let required = root_disk_bytes
        .saturating_add(squashfs_bytes)
        .saturating_add(SAFETY_HEADROOM_BYTES);
    if free < required {
        let free_gb = free / (1024 * 1024 * 1024);
        let req_gb = required / (1024 * 1024 * 1024);
        return Err(InstallerError::DiskOperation(format!(
            "Insufficient free space on {}:\\ — have {} GB, need {} GB (root.disk + squashfs + 2 GB headroom).",
            drive_letter.trim_end_matches(':'),
            free_gb,
            req_gb
        )));
    }
    Ok(())
}

pub fn create_preallocated_root_disk(
    path: &Path,
    size_bytes: u64,
) -> Result<(), InstallerError> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            InstallerError::Io(format!("existing root.disk removal failed: {}", e))
        })?;
    }
    // Remove a stale format marker from any previous installation so the
    // first boot knows this image is raw and must be formatted.
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_file(parent.join(FORMAT_MARKER_FILENAME));
    }

    let create = Command::new("fsutil")
        .args([
            "file",
            "createnew",
            &path.to_string_lossy(),
            &size_bytes.to_string(),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| InstallerError::Io(format!("fsutil createnew spawn failed: {}", e)))?;
    if !create.status.success() {
        return Err(InstallerError::ImageOperation(format!(
            "fsutil createnew failed: {}",
            String::from_utf8_lossy(&create.stderr).trim()
        )));
    }

    let valid = Command::new("fsutil")
        .args([
            "file",
            "setvaliddata",
            &path.to_string_lossy(),
            &size_bytes.to_string(),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| InstallerError::Io(format!("fsutil setvaliddata spawn failed: {}", e)))?;
    if !valid.status.success() {
        return Err(InstallerError::ImageOperation(format!(
            "fsutil setvaliddata failed: {}",
            String::from_utf8_lossy(&valid.stderr).trim()
        )));
    }

    // `setvaliddata` exposes whatever stale bytes were previously on disk
    // inside the file. Zero the head and tail so no leftover filesystem
    // signature (ext4/btrfs/zfs superblocks etc.) can be misdetected by
    // blkid/mount inside the initramfs.
    const WIPE_CHUNK: usize = 1024 * 1024;
    const WIPE_HEAD_BYTES: u64 = 8 * 1024 * 1024;
    const WIPE_TAIL_BYTES: u64 = 4 * 1024 * 1024;

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| InstallerError::Io(format!("root.disk open failed: {}", e)))?;
    let zeros = vec![0u8; WIPE_CHUNK];

    let head = WIPE_HEAD_BYTES.min(size_bytes);
    let mut written: u64 = 0;
    while written < head {
        let n = ((head - written) as usize).min(WIPE_CHUNK);
        f.write_all(&zeros[..n])
            .map_err(|e| InstallerError::Io(format!("root.disk head wipe failed: {}", e)))?;
        written += n as u64;
    }

    let tail = WIPE_TAIL_BYTES.min(size_bytes.saturating_sub(head));
    if tail > 0 {
        f.seek(SeekFrom::Start(size_bytes - tail))
            .map_err(|e| InstallerError::Io(format!("root.disk seek failed: {}", e)))?;
        let mut written: u64 = 0;
        while written < tail {
            let n = ((tail - written) as usize).min(WIPE_CHUNK);
            f.write_all(&zeros[..n])
                .map_err(|e| InstallerError::Io(format!("root.disk tail wipe failed: {}", e)))?;
            written += n as u64;
        }
    }

    f.sync_all()
        .map_err(|e| InstallerError::Io(format!("root.disk sync failed: {}", e)))?;

    Ok(())
}

pub fn place_boot_payload_on_host(
    layout: &HostImageLayout,
    kernel_src: &Path,
    initrd_src: &Path,
    overlay_src: &Path,
) -> Result<(), InstallerError> {
    std::fs::create_dir_all(&layout.boot_dir)
        .map_err(|e| InstallerError::Io(format!("host boot dir create failed: {}", e)))?;
    std::fs::copy(kernel_src, &layout.host_kernel).map_err(|e| {
        InstallerError::Io(format!(
            "kernel copy to host failed ({} -> {}): {}",
            kernel_src.display(),
            layout.host_kernel.display(),
            e
        ))
    })?;
    std::fs::copy(initrd_src, &layout.host_initrd).map_err(|e| {
        InstallerError::Io(format!(
            "initrd copy to host failed ({} -> {}): {}",
            initrd_src.display(),
            layout.host_initrd.display(),
            e
        ))
    })?;
    std::fs::copy(overlay_src, &layout.host_overlay).map_err(|e| {
        InstallerError::Io(format!(
            "overlay copy to host failed ({} -> {}): {}",
            overlay_src.display(),
            layout.host_overlay.display(),
            e
        ))
    })?;
    Ok(())
}

/// Chunked copy so the caller can report real progress instead of the UI
/// sitting frozen for the several-GB squashfs transfer.
pub fn copy_squashfs_from_iso(
    iso_drive_letter: &str,
    squashfs_rel: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64, InstallerError> {
    use std::io::Read;

    let src = format!(
        "{}:\\{}",
        iso_drive_letter.trim_end_matches(':'),
        squashfs_rel.replace('/', "\\")
    );
    let total = std::fs::metadata(&src)
        .map(|m| m.len())
        .map_err(|e| InstallerError::Io(format!("squashfs stat failed ({}): {}", src, e)))?;

    let mut reader = std::fs::File::open(&src)
        .map_err(|e| InstallerError::Io(format!("squashfs open failed ({}): {}", src, e)))?;
    let mut writer = std::fs::File::create(dest)
        .map_err(|e| InstallerError::Io(format!("squashfs dest create failed: {}", e)))?;

    const CHUNK: usize = 16 * 1024 * 1024;
    let mut buf = vec![0u8; CHUNK];
    let mut done: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| InstallerError::Io(format!("squashfs read failed: {}", e)))?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| InstallerError::Io(format!("squashfs write failed: {}", e)))?;
        done += n as u64;
        progress(done, total);
    }
    writer
        .sync_all()
        .map_err(|e| InstallerError::Io(format!("squashfs sync failed: {}", e)))?;
    Ok(done)
}

pub fn read_squashfs_size(iso_drive_letter: &str, squashfs_rel: &str) -> Result<u64, InstallerError> {
    let src = format!(
        "{}:\\{}",
        iso_drive_letter.trim_end_matches(':'),
        squashfs_rel.replace('/', "\\")
    );
    std::fs::metadata(&src)
        .map(|m| m.len())
        .map_err(|e| InstallerError::Io(format!("squashfs stat failed ({}): {}", src, e)))
}

pub fn get_ntfs_volume_serial(drive_letter: &str) -> Result<String, InstallerError> {
    let letter = drive_letter.trim_end_matches(':');
    let script = format!(
        r#"(Get-CimInstance -ClassName Win32_LogicalDisk -Filter "DeviceID='{}:'" -ErrorAction Stop).VolumeSerialNumber"#,
        letter
    );
    let raw = run_powershell(&script)?;
    let serial: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_uppercase();
    if serial.is_empty() {
        return Err(InstallerError::DiskOperation(format!(
            "NTFS volume serial query failed for {}:",
            letter
        )));
    }
    Ok(serial)
}

pub fn write_provisioning_config(
    layout: &HostImageLayout,
    username: &str,
    password: &str,
    hostname: &str,
    locale: &str,
    timezone: &str,
    host_serial: &str,
) -> Result<(), InstallerError> {
    let body = format!(
        "NEXTOS_USERNAME={}\nNEXTOS_PASSWORD={}\nNEXTOS_HOSTNAME={}\nNEXTOS_LOCALE={}\nNEXTOS_TIMEZONE={}\nNEXTOS_HOST_SERIAL={}\nNEXTOS_ROOT_DISK_PATH={}\nNEXTOS_SQUASHFS_PATH={}\n",
        escape_conf(username),
        escape_conf(password),
        escape_conf(hostname),
        escape_conf(locale),
        escape_conf(timezone),
        escape_conf(host_serial),
        layout.root_disk_unix_path,
        layout.squashfs_unix_path,
    );
    std::fs::write(&layout.config, body.replace("\r\n", "\n").as_bytes())
        .map_err(|e| InstallerError::Io(format!("provisioning config write failed: {}", e)))?;
    Ok(())
}

fn escape_conf(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "")
        .replace('\r', "")
}

pub fn remove_host_artifacts(layout: &HostImageLayout) {
    let _ = std::fs::remove_file(&layout.root_disk);
    let _ = std::fs::remove_file(&layout.squashfs);
    let _ = std::fs::remove_file(&layout.config);
    let _ = std::fs::remove_file(&layout.host_kernel);
    let _ = std::fs::remove_file(&layout.host_initrd);
    let _ = std::fs::remove_file(&layout.host_overlay);
    let _ = std::fs::remove_file(layout.host_dir.join(FORMAT_MARKER_FILENAME));
    let _ = std::fs::remove_dir(&layout.boot_dir);
    let _ = std::fs::remove_dir(&layout.host_dir);
}
