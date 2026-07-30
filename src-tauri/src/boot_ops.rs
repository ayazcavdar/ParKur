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

fn is_pe_efi(path: &Path) -> bool {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut mag = [0u8; 2];
    if f.read(&mut mag).ok() != Some(2) {
        return false;
    }
    // PE/COFF "MZ" — reject empty stubs and random non-EFI files.
    mag == [0x4D, 0x5A] && std::fs::metadata(path).map(|m| m.len() >= 1024).unwrap_or(false)
}

fn try_copy_efi_file(src: &Path, esp_nextos: &str, copied: &mut Vec<String>) {
    let Some(name_os) = src.file_name() else {
        return;
    };
    let name_lower = name_os.to_string_lossy().to_lowercase();
    if !src.is_file() || !name_lower.ends_with(".efi") || copied.contains(&name_lower) {
        return;
    }
    if !is_pe_efi(src) {
        return;
    }
    let dest = format!("{}\\{}", esp_nextos, name_os.to_string_lossy());
    if std::fs::copy(src, &dest).is_ok() {
        copied.push(name_lower);
    }
}

fn collect_efi_from_dir(dir: &Path, esp_nextos: &str, copied: &mut Vec<String>, depth: u32) {
    if depth > 4 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_efi_from_dir(&path, esp_nextos, copied, depth + 1);
        } else {
            try_copy_efi_file(&path, esp_nextos, copied);
        }
    }
}

/// Windows Mount-DiskImage sometimes hides `/EFI/boot/*.efi` on hybrid ISOs.
/// Pardus/Debian still ship a FAT `efi.img` we can mount separately.
fn collect_efi_from_efi_img(
    iso_drive_letter: &str,
    esp_nextos: &str,
    copied: &mut Vec<String>,
) -> Result<(), InstallerError> {
    let iso_root = format!("{}:\\", iso_drive_letter.trim_end_matches(':'));
    let candidates = [
        format!("{}efi.img", iso_root),
        format!("{}boot\\grub\\efi.img", iso_root),
        format!("{}EFI\\boot\\efi.img", iso_root),
    ];
    let img_src = candidates.into_iter().find(|p| Path::new(p).is_file());
    let Some(img_src) = img_src else {
        return Ok(());
    };

    let temp = std::env::temp_dir().join(format!(
        "parkur-efi-{}.img",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::copy(&img_src, &temp).map_err(|e| {
        InstallerError::BootloaderConfig(format!("efi.img copy failed: {}", e))
    })?;
    let temp_str = temp.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"
        $path = '{temp}'
        $img = Mount-DiskImage -ImagePath $path -PassThru -ErrorAction Stop
        $letter = $null
        for ($i = 0; $i -lt 40; $i++) {{
            $vol = $img | Get-Volume -ErrorAction SilentlyContinue
            if ($vol -and $vol.DriveLetter) {{ $letter = [string]$vol.DriveLetter; break }}
            Start-Sleep -Milliseconds 250
        }}
        if (-not $letter) {{ throw "efi.img mounted but no drive letter" }}
        $letter
        "#,
        temp = temp_str
    );

    let mount_result = run_powershell(&script);
    let letter = match mount_result {
        Ok(out) => out.trim().to_string(),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(InstallerError::BootloaderConfig(format!(
                "efi.img mount failed: {}",
                e
            )));
        }
    };

    if letter.len() == 1 {
        let efi_root = format!("{}:\\EFI", letter);
        collect_efi_from_dir(Path::new(&efi_root), esp_nextos, copied, 0);
        // Some images put bootx64.efi at the volume root
        if let Ok(entries) = std::fs::read_dir(format!("{}:\\", letter)) {
            for entry in entries.flatten() {
                try_copy_efi_file(&entry.path(), esp_nextos, copied);
            }
        }
    }

    let _ = run_powershell(&format!(
        "Dismount-DiskImage -ImagePath '{}' -ErrorAction SilentlyContinue",
        temp_str
    ));
    let _ = std::fs::remove_file(&temp);
    Ok(())
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
    let iso_root = format!("{}:\\", iso_drive_letter.trim_end_matches(':'));
    let esp_nextos = format!("{}:\\{}", esp_letter, ESP_NEXTOS_DIR);
    let _ = std::fs::create_dir_all(&esp_nextos);

    let mut copied: Vec<String> = Vec::new();

    // 1) Direct known paths (Pardus/Debian live: /EFI/boot/{boot,grub,mm}x64.efi)
    for rel in [
        "EFI\\boot\\bootx64.efi",
        "EFI\\boot\\grubx64.efi",
        "EFI\\boot\\mmx64.efi",
        "EFI\\boot\\shimx64.efi",
        "EFI\\BOOT\\BOOTX64.EFI",
        "EFI\\BOOT\\grubx64.efi",
        "EFI\\debian\\shimx64.efi",
        "EFI\\debian\\grubx64.efi",
        "EFI\\pardus\\shimx64.efi",
        "EFI\\pardus\\grubx64.efi",
    ] {
        try_copy_efi_file(Path::new(&format!("{}{}", iso_root, rel)), &esp_nextos, &mut copied);
    }

    // 2) Walk any EFI directory Windows exposes on the mounted ISO
    for dir in [
        format!("{}EFI", iso_root),
        format!("{}efi", iso_root),
    ] {
        collect_efi_from_dir(Path::new(&dir), &esp_nextos, &mut copied, 0);
    }

    // 3) Fallback: mount the hybrid ISO's FAT efi.img (common when Windows
    // hides /EFI/boot on the ISO9660 session).
    if copied.is_empty() {
        let _ = collect_efi_from_efi_img(iso_drive_letter, &esp_nextos, &mut copied);
    }

    if copied.is_empty() {
        return Err(InstallerError::coded(
            InstallerError::BootloaderConfig,
            "ERR_NO_UEFI_EFI",
            &[],
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
        return Err(InstallerError::coded(
            InstallerError::BootloaderConfig,
            "ERR_NO_BOOT_APP",
            &[],
        ));
    };

    // Also populate EFI\BOOT\ so the machine boots NextOS from the removable
    // fallback path when NVRAM entries are cleared.
    // IMPORTANT: shim (BOOTX64.EFI) AND grubx64.efi must be in the SAME
    // directory, because shim looks for grubx64.efi next to itself.
    let esp_boot_dir = format!("{}:\\{}", esp_letter, ESP_REMOVABLE_DIR);
    let _ = std::fs::create_dir_all(&esp_boot_dir);

    let primary_src = format!("{}\\{}", esp_nextos, boot_name);
    let removable_dest = format!("{}\\{}", esp_boot_dir, ESP_REMOVABLE_FILE);
    backup_esp_file_once(&removable_dest);
    let _ = std::fs::copy(&primary_src, &removable_dest);

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
