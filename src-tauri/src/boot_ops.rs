use crate::error::InstallerError;
use crate::image_ops::BCD_TIMEOUT_BACKUP_FILENAME;
use crate::util::{extract_first_guid, run_powershell, CREATE_NO_WINDOW};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

pub const ESP_NEXTOS_DIR: &str = "EFI\\NextOS";
pub const ESP_REMOVABLE_DIR: &str = "EFI\\BOOT";
pub const ESP_REMOVABLE_FILE: &str = "BOOTX64.EFI";
pub const NEXTOS_BCD_DESCRIPTION: &str = "NextOS";
/// Suffix for pre-install backups of ESP files we overwrite (original
/// fallback bootloader, foreign grub.cfg). Restored on uninstall.
pub const ESP_BACKUP_SUFFIX: &str = ".parkur-backup";

/// Back up `path` to `path + ESP_BACKUP_SUFFIX` before we overwrite it.
/// The backup is only written once — on reinstall the existing backup still
/// holds the true original, so it must not be clobbered with our own file.
fn backup_esp_file_once(path: &str) {
    let backup = format!("{}{}", path, ESP_BACKUP_SUFFIX);
    if Path::new(path).exists() && !Path::new(&backup).exists() {
        let _ = std::fs::copy(path, &backup);
    }
}

/// Undo `backup_esp_file_once`: restore the original if a backup exists,
/// otherwise remove the file we created (there was no original).
fn restore_esp_file(path: &str) {
    let backup = format!("{}{}", path, ESP_BACKUP_SUFFIX);
    if Path::new(&backup).exists() {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::rename(&backup, path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum BootMode {
    UEFI,
    LegacyBIOS,
}

pub fn detect_boot_mode() -> Result<BootMode, InstallerError> {
    let script = r#"
        $fwType = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control' -Name 'PEFirmwareType' -ErrorAction SilentlyContinue).PEFirmwareType
        if ($fwType -eq 2) { "UEFI" }
        elseif ($fwType -eq 1) { "BIOS" }
        else {
            if (Test-Path 'HKLM:\SYSTEM\CurrentControlSet\Control\SecureBoot') { "UEFI" }
            else { "BIOS" }
        }
    "#;
    let output = run_powershell(script)?;
    match output.trim() {
        "UEFI" => Ok(BootMode::UEFI),
        "BIOS" => Ok(BootMode::LegacyBIOS),
        _ => Ok(BootMode::UEFI),
    }
}

pub fn mount_esp() -> Result<String, InstallerError> {
    let script = r#"
        $espPart = Get-Partition | Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' } | Select-Object -First 1
        if (-not $espPart) { throw "EFI System Partition not found" }
        if ($espPart.DriveLetter) {
            $espPart.DriveLetter
            return
        }
        $targetLetter = $null
        foreach ($l in @('S','R','Q','P')) {
            if (-not (Test-Path "${l}:\")) {
                $targetLetter = $l
                break
            }
        }
        if (-not $targetLetter) { throw "No drive letter available for ESP" }
        Add-PartitionAccessPath -DiskNumber $espPart.DiskNumber -PartitionNumber $espPart.PartitionNumber -AccessPath "${targetLetter}:\" -ErrorAction Stop
        Start-Sleep -Milliseconds 1000
        $targetLetter
    "#;
    let output = run_powershell(script)
        .map_err(|e| InstallerError::BootloaderConfig(format!("ESP mount failed: {}", e)))?;
    let letter = output.trim().to_string();
    if letter.len() != 1 {
        return Err(InstallerError::BootloaderConfig(format!(
            "Invalid ESP drive letter: '{}'",
            letter
        )));
    }
    Ok(letter)
}

/// Copy the complete EFI boot chain from the ISO to the ESP.
///
/// Returns the EFI path string (e.g. `\EFI\NextOS\shimx64.efi`) that should be
/// registered as the UEFI boot entry.  Prefers the Microsoft-signed shim so the
/// installer works with Secure Boot enabled.  Falls back to grubx64.efi if no
/// shim is present on the ISO.
pub fn copy_efi_payload_from_iso(
    iso_drive_letter: &str,
    esp_letter: &str,
) -> Result<String, InstallerError> {
    let iso_root = format!("{}:\\", iso_drive_letter);

    // Directories on the ISO that may contain EFI binaries, in search order
    let iso_efi_dirs = [
        format!("{}EFI\\BOOT", iso_root),
        format!("{}EFI\\Boot", iso_root),
        format!("{}EFI\\boot", iso_root),
        format!("{}EFI\\debian", iso_root),
    ];

    let esp_nextos = format!("{}:\\{}", esp_letter, ESP_NEXTOS_DIR);

    // Create destination directory (direct fs call — the process is elevated,
    // no need to spawn PowerShell for a mkdir)
    let _ = std::fs::create_dir_all(&esp_nextos);

    // Collect lowercase names of .efi files that were successfully copied
    let mut copied: Vec<String> = Vec::new();
    for dir in &iso_efi_dirs {
        let dir_path = Path::new(dir);
        if !dir_path.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let src = entry.path();
                let name_os = entry.file_name();
                let name_lower = name_os.to_string_lossy().to_lowercase();
                if !src.is_file() || !name_lower.ends_with(".efi") {
                    continue;
                }
                // Verify PE/COFF magic
                if let Ok(data) = std::fs::read(&src) {
                    if data.len() < 1024
                        || data.first() != Some(&0x4D)
                        || data.get(1) != Some(&0x5A)
                    {
                        continue;
                    }
                    let dest = format!("{}\\{}", esp_nextos, name_os.to_string_lossy());
                    // Don't overwrite a file we already placed (first dir wins)
                    if copied.contains(&name_lower) {
                        continue;
                    }
                    if std::fs::copy(&src, &dest).is_ok() {
                        copied.push(name_lower);
                    }
                }
            }
        }
    }

    if copied.is_empty() {
        return Err(InstallerError::BootloaderConfig(
            "No valid EFI binaries found in ISO. The ISO must support 64-bit UEFI.".into(),
        ));
    }

    // Pick the boot EFI to register.
    // Prefer shimx64.efi → BOOTX64.EFI (shim, Microsoft-signed, works with Secure Boot).
    // Shim will automatically load grubx64.efi from the same directory.
    // Fall back to grubx64.efi when no shim is available.
    let boot_name = if copied.contains(&"shimx64.efi".to_string()) {
        "shimx64.efi"
    } else if copied.contains(&"bootx64.efi".to_string()) {
        // BOOTX64.EFI on Debian/Pardus ISOs is shimx64.efi
        "bootx64.efi"
    } else if copied.contains(&"grubx64.efi".to_string()) {
        "grubx64.efi"
    } else {
        // Last resort: first file we copied
        return Err(InstallerError::BootloaderConfig(
            "No recognisable EFI boot application found in ISO.".into(),
        ));
    };

    // Also populate EFI\BOOT\ so the machine boots NextOS from the removable
    // fallback path when NVRAM entries are cleared.
    // IMPORTANT: shim (BOOTX64.EFI) AND grubx64.efi must be in the SAME
    // directory, because shim looks for grubx64.efi next to itself.
    let esp_boot_dir = format!("{}:\\{}", esp_letter, ESP_REMOVABLE_DIR);
    let _ = std::fs::create_dir_all(&esp_boot_dir);

    // Copy the chosen boot EFI as BOOTX64.EFI (the fallback entry).
    // The machine's original fallback bootloader is backed up first so
    // uninstall can put it back.
    let primary_src = format!("{}\\{}", esp_nextos, boot_name);
    let removable_dest = format!("{}\\{}", esp_boot_dir, ESP_REMOVABLE_FILE);
    backup_esp_file_once(&removable_dest);
    let _ = std::fs::copy(&primary_src, &removable_dest);

    // Copy grubx64.efi to EFI\BOOT\ so shim can find it when loaded from there
    if boot_name != "grubx64.efi" {
        let grub_src = format!("{}\\grubx64.efi", esp_nextos);
        let grub_fallback = format!("{}\\grubx64.efi", esp_boot_dir);
        if Path::new(&grub_src).exists() {
            backup_esp_file_once(&grub_fallback);
            let _ = std::fs::copy(&grub_src, &grub_fallback);
        }
    }

    Ok(format!("\\{}\\{}", ESP_NEXTOS_DIR, boot_name))
}

pub fn generate_loop_grub_cfg(host_serial: &str) -> String {
    format!(
        r#"set default="0"
set timeout="3"
set timeout_style="menu"

insmod part_gpt
insmod part_msdos
insmod fat
insmod ntfs
if insmod ntfs3; then true; fi
insmod chain
insmod search
insmod search_fs_file
insmod search_fs_uuid
insmod all_video
insmod gfxterm
insmod linux

if loadfont unicode; then
    set gfxmode=auto
    terminal_output gfxterm
elif loadfont $prefix/fonts/unicode.pf2; then
    set gfxmode=auto
    terminal_output gfxterm
fi

# Locate the NTFS host volume containing the NextOS payload.
# search --file sets 'ntfsroot' to the partition device (e.g. hd0,gpt3).
search --no-floppy --set=ntfsroot --file /NextOS/boot/vmlinuz

# NOTE: 'quiet splash' intentionally omitted so the [nextos] initramfs logs
# stay visible while the boot flow is being stabilised. Re-add once stable.
# The overlay is loaded as a SECOND initrd: the kernel extracts both archives
# in order, so the overlay's /init replaces live-boot's /init.
menuentry "NextOS" {{
    linux ($ntfsroot)/NextOS/boot/vmlinuz ro nextos_host_serial={host_ser} nextos_root_disk=/NextOS/root.disk nextos_squashfs=/NextOS/filesystem.squashfs
    initrd ($ntfsroot)/NextOS/boot/initrd.img ($ntfsroot)/NextOS/boot/overlay.cpio.gz
}}

menuentry "Windows" {{
    search --no-floppy --set=winroot --file /EFI/Microsoft/Boot/bootmgfw.efi
    chainloader ($winroot)/EFI/Microsoft/Boot/bootmgfw.efi
}}
"#,
        host_ser = host_serial,
    )
    .replace("\r\n", "\n")
}

pub fn write_grub_cfg(esp_letter: &str, content: &str) -> Result<(), InstallerError> {
    // Write grub.cfg to every location a Debian/Pardus GRUB EFI binary might search.
    // Pardus GRUB 2.12 is built with -p /EFI/debian (standard Debian prefix).
    // We cover all candidates so any variant of the binary finds the config.
    let locations: &[&str] = &[
        // Standard Debian GRUB EFI prefix (-p /EFI/debian)
        "EFI\\debian\\grub.cfg",
        // Pardus may use its own prefix
        "EFI\\pardus\\grub.cfg",
        // Removable / fallback path (when loaded as BOOTX64.EFI from EFI\BOOT)
        "EFI\\BOOT\\grub.cfg",
        // Our primary payload directory
        "EFI\\NextOS\\grub.cfg",
        // Legacy / alternative prefix
        "boot\\grub\\grub.cfg",
    ];

    // Direct writes — the elevated process can write to the mounted ESP
    // without spawning one PowerShell per file.
    let lf = crate::util::to_lf(content);
    for rel in locations {
        let dest = format!("{}:\\{}", esp_letter, rel);
        if let Some(parent) = Path::new(&dest).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // A grub.cfg may belong to a real Debian/Pardus installation on this
        // machine — keep the original so uninstall can restore it.
        backup_esp_file_once(&dest);
        // Best-effort — non-fatal if a specific path fails
        let _ = std::fs::write(&dest, lf.as_bytes());
    }

    // At least the primary must succeed
    let primary = format!("{}:\\EFI\\debian\\grub.cfg", esp_letter);
    std::fs::write(&primary, lf.as_bytes()).map_err(|e| {
        InstallerError::BootloaderConfig(format!("grub.cfg write failed ({}): {}", primary, e))
    })?;

    Ok(())
}

/// Register a UEFI firmware boot entry that points to `efi_path` on the ESP.
///
/// `efi_path` must be an absolute EFI path, e.g. `\EFI\NextOS\shimx64.efi`.
/// Uses `/copy {bootmgr}` which creates a proper UEFI firmware-visible entry
/// (unlike `/create /application bootsector` which creates a Windows Boot Manager
/// sub-entry not visible to the UEFI firmware directly).
pub fn register_firmware_entry(esp_letter: &str, efi_path: &str) -> Result<String, InstallerError> {
    let _ = cleanup_nextos_firmware_entries();

    // /copy {bootmgr} is the correct way to create a new UEFI NVRAM boot entry
    // on Windows — it copies the entry type (firmware application) and gives us
    // an entry that the UEFI firmware will actually execute.
    let copy_out = Command::new("bcdedit")
        .args(["/copy", "{bootmgr}", "/d", NEXTOS_BCD_DESCRIPTION])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| InstallerError::BootloaderConfig(format!("bcdedit copy spawn: {}", e)))?;
    if !copy_out.status.success() {
        return Err(InstallerError::BootloaderConfig(format!(
            "bcdedit copy failed: {}",
            String::from_utf8_lossy(&copy_out.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&copy_out.stdout).to_string();
    let guid = extract_first_guid(&stdout).ok_or_else(|| {
        InstallerError::BootloaderConfig(format!("GUID parse failed: {}", stdout.trim()))
    })?;

    run_bcdedit(&["/set", &guid, "device", &format!("partition={}:", esp_letter)])?;
    run_bcdedit(&["/set", &guid, "path", efi_path])?;
    run_bcdedit(&["/set", &guid, "description", NEXTOS_BCD_DESCRIPTION])?;
    // Place NextOS FIRST in the firmware boot order
    run_bcdedit(&["/set", "{fwbootmgr}", "displayorder", &guid, "/addfirst"])?;
    run_bcdedit(&["/timeout", "5"])?;

    Ok(guid)
}

fn run_bcdedit(args: &[&str]) -> Result<String, InstallerError> {
    let output = Command::new("bcdedit")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| InstallerError::BootloaderConfig(format!("bcdedit spawn: {}", e)))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::BootloaderConfig(format!(
            "bcdedit {:?} failed: {} {}",
            args,
            stdout.trim(),
            stderr.trim()
        )));
    }
    Ok(stdout)
}

/// Remember the firmware boot-menu timeout before we change it globally.
pub fn backup_firmware_boot_timeout_to_host(host_dir: &Path) -> Result<(), InstallerError> {
    let backup_path = host_dir.join(BCD_TIMEOUT_BACKUP_FILENAME);
    if backup_path.exists() {
        return Ok(());
    }
    if let Some(timeout) = read_firmware_boot_timeout() {
        std::fs::write(&backup_path, timeout).map_err(|e| {
            InstallerError::BootloaderConfig(format!("bcd timeout backup write failed: {}", e))
        })?;
    }
    Ok(())
}

pub fn restore_firmware_boot_timeout_from_host(host_dir: &Path) -> Result<(), InstallerError> {
    let backup_path = host_dir.join(BCD_TIMEOUT_BACKUP_FILENAME);
    if !backup_path.exists() {
        return Ok(());
    }
    let timeout = std::fs::read_to_string(&backup_path).map_err(|e| {
        InstallerError::BootloaderConfig(format!("bcd timeout backup read failed: {}", e))
    })?;
    let timeout = timeout.trim();
    if !timeout.is_empty() {
        run_bcdedit(&["/timeout", timeout])?;
    }
    let _ = std::fs::remove_file(backup_path);
    Ok(())
}

fn read_firmware_boot_timeout() -> Option<String> {
    let output = run_bcdedit(&["/enum", "{bootmgr}"]).ok()?;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.to_ascii_lowercase().starts_with("timeout") {
            if let Some(val) = trimmed.split_whitespace().last() {
                if val.chars().all(|c| c.is_ascii_digit()) {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

pub fn cleanup_nextos_firmware_entries() -> Result<Vec<String>, InstallerError> {
    let script = r#"
        $output = bcdedit /enum firmware 2>&1 | Out-String
        $entries = @()
        $currentId = $null
        foreach ($line in ($output -split "`n")) {
            $line = $line.Trim()
            if ($line -match '^identifier\s+(.+)$') {
                $currentId = $Matches[1].Trim()
            }
            if ($line -match '^description\s+(.+)$') {
                $desc = $Matches[1].Trim()
                if ($currentId -and ($desc -like '*NextOS*' -or $desc -like '*Next OS*' -or $desc -like '*Next-OS*')) {
                    if ($currentId -ne '{bootmgr}' -and $currentId -ne '{fwbootmgr}') {
                        $entries += $currentId
                    }
                }
            }
        }
        if ($entries.Count -eq 0) { "NONE" }
        else { $entries -join ";" }
    "#;
    let output = run_powershell(script)?;
    let trimmed = output.trim();
    if trimmed == "NONE" || trimmed.is_empty() {
        return Ok(vec![]);
    }
    let mut deleted = Vec::new();
    for guid in trimmed.split(';') {
        let guid = guid.trim();
        if run_bcdedit(&["/delete", guid, "/f"]).is_ok() {
            deleted.push(guid.to_string());
        }
    }
    Ok(deleted)
}

pub fn cleanup_esp_payload(esp_letter: &str) -> Result<(), InstallerError> {
    // Our own payload directory goes away entirely.
    let nextos_dir = format!("{}:\\{}", esp_letter, ESP_NEXTOS_DIR);
    if Path::new(&nextos_dir).exists() {
        let _ = std::fs::remove_dir_all(&nextos_dir);
    }

    // Files we overwrote elsewhere: restore the pre-install backup when one
    // exists, otherwise just delete what we created.
    let overwritten = [
        format!("{}:\\{}\\{}", esp_letter, ESP_REMOVABLE_DIR, ESP_REMOVABLE_FILE),
        format!("{}:\\{}\\grubx64.efi", esp_letter, ESP_REMOVABLE_DIR),
        format!("{}:\\{}\\grub.cfg", esp_letter, ESP_REMOVABLE_DIR),
        format!("{}:\\EFI\\debian\\grub.cfg", esp_letter),
        format!("{}:\\EFI\\pardus\\grub.cfg", esp_letter),
        format!("{}:\\boot\\grub\\grub.cfg", esp_letter),
    ];
    for path in &overwritten {
        restore_esp_file(path);
    }

    // Remove directories we may have created if they are now empty
    // (remove_dir fails on non-empty dirs, which is exactly what we want).
    for dir in [
        format!("{}:\\EFI\\debian", esp_letter),
        format!("{}:\\EFI\\pardus", esp_letter),
        format!("{}:\\boot\\grub", esp_letter),
        format!("{}:\\boot", esp_letter),
    ] {
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

/// Set the one-time UEFI BootNext (bootsequence) override so the very next
/// reboot goes directly into NextOS regardless of persistent BootOrder.
/// `guid` is the BCD identifier returned by `register_firmware_entry`.
pub fn set_nextos_bootsequence(guid: &str) -> Result<(), InstallerError> {
    if guid.is_empty() {
        return Ok(());
    }
    run_bcdedit(&["/set", "{fwbootmgr}", "bootsequence", guid])?;
    Ok(())
}

pub fn reboot_system(nextos_guid: &str) -> Result<(), InstallerError> {
    // Best-effort: one-shot UEFI BootNext override so this reboot goes to NextOS
    let _ = set_nextos_bootsequence(nextos_guid);

    let output = Command::new("shutdown")
        .args(["/r", "/t", "3", "/c", "Booting into NextOS"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| InstallerError::BootloaderConfig(format!("reboot spawn failed: {}", e)))?;
    if !output.status.success() {
        return Err(InstallerError::BootloaderConfig(format!(
            "reboot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
