
use crate::disk_ops::run_powershell;
use crate::error::InstallerError;
use crate::iso_ops::LinuxKernelInfo;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum BootMode {
    UEFI,
    LegacyBIOS,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BootConfigResult {
    pub boot_mode: BootMode,
    pub bcd_entry_guid: String,
    pub esp_path: Option<String>,
    pub message: String,
}

pub fn detect_boot_mode() -> Result<BootMode, InstallerError> {
    let script = r#"
        $fwType = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control' -Name 'PEFirmwareType' -ErrorAction SilentlyContinue).PEFirmwareType
        if ($fwType -eq 2) { "UEFI" }
        elseif ($fwType -eq 1) { "BIOS" }
        else {
            # Alternatif kontrol: Secure Boot varlÄ±ÄŸÄ± UEFI demektir
            if (Test-Path 'HKLM:\SYSTEM\CurrentControlSet\Control\SecureBoot') { "UEFI" }
            else { "BIOS" }
        }
    "#;

    let output = run_powershell(script)?;
    let mode = output.trim();

    match mode {
        "UEFI" => {
            println!("[BOOT] Boot modu: UEFI");
            Ok(BootMode::UEFI)
        }
        "BIOS" => {
            println!("[BOOT] Boot modu: Legacy BIOS");
            Ok(BootMode::LegacyBIOS)
        }
        _ => {
            println!("[BOOT] Boot modu belirlenemedi, UEFI varsayÄ±lÄ±yor. Ã‡Ä±ktÄ±: {}", mode);
            Ok(BootMode::UEFI)
        }
    }
}

pub fn mount_esp() -> Result<String, InstallerError> {
    let check_script = r#"
        $espVol = Get-Partition | Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' } | Select-Object -First 1
        if ($espVol -and $espVol.DriveLetter) {
            $espVol.DriveLetter
        } else {
            "NOT_MOUNTED"
        }
    "#;

    let check_result = run_powershell(check_script)?;
    let check_trimmed = check_result.trim();

    if check_trimmed != "NOT_MOUNTED" && check_trimmed.len() == 1 {
        println!("[BOOT] ESP zaten monte: sÃ¼rÃ¼cÃ¼ {}", check_trimmed);
        return Ok(check_trimmed.to_string());
    }

    let mount_script = r#"
        $targetLetter = $null
        $testLetters = @('S','R','Q','P')
        foreach ($l in $testLetters) {
            if (-not (Test-Path "${l}:\")) {
                $targetLetter = $l
                break
            }
        }
        if (-not $targetLetter) { throw "ESP icin uygun surucu harfi bulunamadi." }

        $espPart = Get-Partition | Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' } | Select-Object -First 1
        if (-not $espPart) { throw "EFI System Partition bulunamadi. Sistem UEFI modunda mi?" }

        $diskNum = $espPart.DiskNumber
        $partNum = $espPart.PartitionNumber
        Add-PartitionAccessPath -DiskNumber $diskNum -PartitionNumber $partNum -AccessPath "${targetLetter}:\" -ErrorAction Stop

        Start-Sleep -Milliseconds 1000
        $targetLetter
    "#;

    let output = run_powershell(mount_script).map_err(|e| {
        InstallerError::BootloaderConfig(format!("ESP monte edilemedi: {}", e))
    })?;

    let letter = output.trim().to_string();
    if letter.len() != 1 {
        return Err(InstallerError::BootloaderConfig(format!(
            "ESP monte edildi ancak sÃ¼rÃ¼cÃ¼ harfi alÄ±namadÄ±. Ã‡Ä±ktÄ±: '{}'",
            letter
        )));
    }

    println!("[BOOT] ESP monte edildi: sÃ¼rÃ¼cÃ¼ {}", letter);
    Ok(letter)
}

fn validate_efi_binary(path: &str) -> Result<(), InstallerError> {
    let metadata = std::fs::metadata(path).map_err(|e| {
        InstallerError::BootloaderConfig(format!(
            "EFI dosyasÄ± okunamadÄ± ({}): {}", path, e
        ))
    })?;

    let file_size = metadata.len();

    if file_size == 0 {
        return Err(InstallerError::BootloaderConfig(format!(
            "EFI dosyasÄ± 0 byte! Kopyalama sÄ±rasÄ±nda veri kaybolmuÅŸ: {}", path
        )));
    }

    if file_size < 10_000 {
        return Err(InstallerError::BootloaderConfig(format!(
            "EFI dosyasÄ± anormal ÅŸekilde kÃ¼Ã§Ã¼k ({} byte). Bozuk olabilir: {}",
            file_size, path
        )));
    }

    let header = std::fs::read(path).map_err(|e| {
        InstallerError::BootloaderConfig(format!(
            "EFI dosyasÄ± okunamadÄ± ({}): {}", path, e
        ))
    })?;

    if header.len() < 2 || header[0] != 0x4D || header[1] != 0x5A {
        return Err(InstallerError::BootloaderConfig(format!(
            "EFI dosyasÄ± geÃ§erli bir PE binary deÄŸil (MZ header eksik). \
             Ä°lk 2 byte: {:02X} {:02X}. Dosya: {}",
            header.get(0).unwrap_or(&0),
            header.get(1).unwrap_or(&0),
            path
        )));
    }

    println!(
        "[BOOT] EFI doÄŸrulama BAÅARILI: {} ({} KB, PE header OK)",
        path,
        file_size / 1024
    );
    Ok(())
}

fn copy_and_validate_efi(source: &str, dest: &str) -> Result<(), InstallerError> {
    validate_efi_binary(source)?;

    // PowerShell ile kopyalama — std::fs::write EFI bölümünde "Erişim engellendi" verir
    let ps_script = format!(
        r#"Copy-Item -Path '{}' -Destination '{}' -Force -ErrorAction Stop"#,
        source, dest
    );
    run_powershell(&ps_script).map_err(|e| {
        InstallerError::BootloaderConfig(format!(
            "EFI dosyası PowerShell ile kopyalanamadı ({} -> {}): {}",
            source, dest, e
        ))
    })?;

    // Boyut doğrulaması
    let source_size = std::fs::metadata(source)
        .map_err(|e| InstallerError::Io(format!("Kaynak EFI okunamadı: {}", e)))?
        .len();
    let dest_size = std::fs::metadata(dest)
        .map_err(|e| InstallerError::Io(format!("Hedef EFI okunamadı: {}", e)))?
        .len();

    if source_size != dest_size {
        let _ = run_powershell(&format!("Remove-Item -Path '{}' -Force", dest));
        return Err(InstallerError::BootloaderConfig(format!(
            "EFI dosya boyutu uyuşmuyor! Kaynak: {} byte, Hedef: {} byte. \
             Kopyalama bozuk, dosya silindi.",
            source_size, dest_size
        )));
    }

    validate_efi_binary(dest)?;

    println!(
        "[BOOT] EFI kopyalama + doğrulama OK: {} -> {} ({} KB)",
        source, dest, dest_size / 1024
    );
    Ok(())
}

pub fn setup_grub_efi(
    iso_partition: &str,
    esp_letter: &str,
    kernel_info: &LinuxKernelInfo,
) -> Result<(), InstallerError> {
    let esp_grub_dir = format!("{}:\\EFI\\NextOS", esp_letter);
    let iso_root = format!("{}:\\", iso_partition);

    // PowerShell ile dizin oluştur — std::fs::create_dir_all EFI bölümünde izin hatası verir
    let mkdir_script = format!(
        "New-Item -Path '{}' -ItemType Directory -Force -ErrorAction Stop | Out-Null",
        esp_grub_dir
    );
    run_powershell(&mkdir_script).map_err(|e| {
        InstallerError::BootloaderConfig(format!(
            "ESP üzerinde dizin oluşturulamadı ({}): {}", esp_grub_dir, e
        ))
    })?;

    println!("[BOOT] ISO EFI dizini taranÄ±yor: {}EFI\\", iso_root);
    log_directory_tree(&format!("{}EFI", iso_root), 0, 3);

    let grub_candidates = [
        format!("{}EFI\\boot\\grubx64.efi", iso_root),
        format!("{}EFI\\Boot\\grubx64.efi", iso_root),
        format!("{}EFI\\BOOT\\GRUBX64.EFI", iso_root),
        format!("{}EFI\\BOOT\\grubx64.efi", iso_root),
        format!("{}EFI\\boot\\bootx64.efi", iso_root),
        format!("{}EFI\\Boot\\bootx64.efi", iso_root),
        format!("{}EFI\\BOOT\\BOOTX64.EFI", iso_root),
        format!("{}EFI\\BOOT\\bootx64.efi", iso_root),
        format!("{}boot\\grub\\x86_64-efi\\grub.efi", iso_root),
    ];

    let mut grub_source: Option<String> = None;
    for candidate in &grub_candidates {
        let path = std::path::Path::new(candidate);
        if path.exists() {
            match validate_efi_binary(candidate) {
                Ok(()) => {
                    grub_source = Some(candidate.clone());
                    println!("[BOOT] GeÃ§erli EFI binary bulundu: {}", candidate);
                    break;
                }
                Err(e) => {
                    println!("[BOOT] UYARI: {} var ama geÃ§erli deÄŸil: {}", candidate, e);
                    continue;
                }
            }
        }
    }

    let grub_source = grub_source.ok_or_else(|| {
        InstallerError::BootloaderConfig(format!(
            "ISO iÃ§eriklerinde geÃ§erli bir GRUB EFI binary'si bulunamadÄ±. \
             Aranan yollar: EFI/boot/grubx64.efi, EFI/boot/bootx64.efi vb. \
             ISO dizini: {}\n\
             LÃ¼tfen 64-bit UEFI destekli bir ISO kullandÄ±ÄŸÄ±nÄ±zdan emin olun.",
            iso_root
        ))
    })?;

    let grub_dest = format!("{}\\grubx64.efi", esp_grub_dir);
    copy_and_validate_efi(&grub_source, &grub_dest)?;

    let efi_boot_src = format!("{}EFI\\boot", iso_root);
    let efi_boot_alt = format!("{}EFI\\Boot", iso_root);
    let efi_boot_upper = format!("{}EFI\\BOOT", iso_root);
    let efi_source_dir = if std::path::Path::new(&efi_boot_src).is_dir() {
        Some(efi_boot_src)
    } else if std::path::Path::new(&efi_boot_alt).is_dir() {
        Some(efi_boot_alt)
    } else if std::path::Path::new(&efi_boot_upper).is_dir() {
        Some(efi_boot_upper)
    } else {
        None
    };

    if let Some(ref src_dir) = efi_source_dir {
        let efi_boot_dest = format!("{}\\boot", esp_grub_dir);
        println!("[BOOT] EFI boot dizini kopyalanÄ±yor: {} -> {}", src_dir, efi_boot_dest);
        let _ = Command::new("robocopy")
            .args([
                src_dir,
                &efi_boot_dest,
                "/E", "/R:2", "/W:1",
                "/NFL", "/NDL", "/NJH", "/NJS",
            ])
            .output();
    }

    let grub_modules_src = format!("{}boot\\grub", iso_root);
    let grub_modules_dest = format!("{}\\grub", esp_grub_dir);
    if std::path::Path::new(&grub_modules_src).is_dir() {
        println!("[BOOT] GRUB modÃ¼lleri kopyalanÄ±yor...");
        let _ = Command::new("robocopy")
            .args([
                &grub_modules_src,
                &grub_modules_dest,
                "/E", "/R:2", "/W:1",
                "/NFL", "/NDL", "/NJH", "/NJS",
            ])
            .output();
    }

    let grub_cfg = format!(
        r#"# Next OS Installer - Otomatik GRUB yapÄ±landÄ±rmasÄ±
# Bu dosya kurulum sonrasÄ± otomatik olarak silinecektir.

set default=0
set timeout=3

insmod all_video
insmod gfxterm
insmod png

menuentry "Next OS - Linux Kurulum" {{
    search --no-floppy --label NextOS_Install --set root
    linux /{kernel} boot=live components quiet splash
    initrd /{initrd}
}}

menuentry "Windows'a Don (Boot Manager)" {{
    exit
}}
"#,
        kernel = kernel_info.kernel_path.replace('\\', "/"),
        initrd = kernel_info.initrd_path.replace('\\', "/"),
    );

    let cfg_locations = [
        format!("{}\\grub.cfg", esp_grub_dir),
        format!("{}\\grub\\grub.cfg", esp_grub_dir),
        format!("{}\\boot\\grub\\grub.cfg", esp_grub_dir),
    ];

    for cfg_path in &cfg_locations {
        if let Some(parent) = std::path::Path::new(cfg_path).parent() {
            let mkdir_ps = format!(
                "New-Item -Path '{}' -ItemType Directory -Force -ErrorAction SilentlyContinue | Out-Null",
                parent.to_string_lossy()
            );
            let _ = run_powershell(&mkdir_ps);
        }
        // grub.cfg yazımını PowerShell Set-Content ile yap
        let escaped_content = grub_cfg.replace("'", "''");
        let write_ps = format!(
            "Set-Content -Path '{}' -Value '{}' -Encoding UTF8 -Force -ErrorAction Stop",
            cfg_path, escaped_content
        );
        run_powershell(&write_ps).map_err(|e| {
            InstallerError::BootloaderConfig(format!(
                "grub.cfg oluşturulamadı ({}): {}", cfg_path, e
            ))
        })?;
        println!("[BOOT] grub.cfg yazıldı: {}", cfg_path);
    }

    println!("[BOOT] GRUB EFI kurulumu tamamlandÄ±");
    Ok(())
}

fn log_directory_tree(path: &str, depth: u32, max_depth: u32) {
    if depth >= max_depth {
        return;
    }
    let indent = "  ".repeat(depth as usize);
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let ftype = if entry.path().is_dir() { "[DIR]" } else { "" };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if entry.path().is_dir() {
                println!("[BOOT]   {}{} {}", indent, ftype, name);
                log_directory_tree(&entry.path().to_string_lossy(), depth + 1, max_depth);
            } else {
                println!("[BOOT]   {}{} ({} KB)", indent, name, size / 1024);
            }
        }
    }
}

pub fn create_bcd_entry(esp_letter: &str) -> Result<String, InstallerError> {
    let create_output = Command::new("bcdedit")
        .args(["/copy", "{bootmgr}", "/d", "Next OS Installer"])
        .output()
        .map_err(|e| {
            InstallerError::BootloaderConfig(format!("bcdedit baÅŸlatÄ±lamadÄ±: {}", e))
        })?;

    let create_stdout = String::from_utf8_lossy(&create_output.stdout).to_string();
    let create_stderr = String::from_utf8_lossy(&create_output.stderr).to_string();

    if !create_output.status.success() {
        return Err(InstallerError::BootloaderConfig(format!(
            "bcdedit /copy {{bootmgr}} hatasÄ±:\nstdout: {}\nstderr: {}",
            create_stdout.trim(),
            create_stderr.trim()
        )));
    }

    let guid = extract_guid(&create_stdout).ok_or_else(|| {
        InstallerError::BootloaderConfig(format!(
            "bcdedit Ã§Ä±ktÄ±sÄ±ndan GUID ayrÄ±ÅŸtÄ±rÄ±lamadÄ±. Ã‡Ä±ktÄ±: '{}'",
            create_stdout.trim()
        ))
    })?;

    println!("[BOOT] BCD firmware application giriÅŸi oluÅŸturuldu: {}", guid);

    run_bcdedit(&["/set", &guid, "device", &format!("partition={}:", esp_letter)])?;

    run_bcdedit(&["/set", &guid, "path", "\\EFI\\NextOS\\grubx64.efi"])?;

    run_bcdedit(&["/set", &guid, "description", "Next OS Installer"])?;

    run_bcdedit(&["/set", "{fwbootmgr}", "displayorder", &guid, "/addfirst"])?;

    run_bcdedit(&["/set", "{fwbootmgr}", "bootsequence", &guid])?;

    println!("[BOOT] BCD yapÄ±landÄ±rmasÄ± tamamlandÄ± (firmware app). GUID: {}", guid);

    Ok(guid)
}

fn run_bcdedit(args: &[&str]) -> Result<String, InstallerError> {
    println!("[BOOT] bcdedit {:?}", args);

    let output = Command::new("bcdedit")
        .args(args)
        .output()
        .map_err(|e| {
            InstallerError::BootloaderConfig(format!(
                "bcdedit komutu Ã§alÄ±ÅŸtÄ±rÄ±lamadÄ± ({:?}): {}", args, e
            ))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(InstallerError::BootloaderConfig(format!(
            "bcdedit {:?} hatasÄ±:\nstdout: {}\nstderr: {}",
            args, stdout.trim(), stderr.trim()
        )));
    }

    println!("[BOOT] bcdedit OK: {}", stdout.trim());
    Ok(stdout)
}

fn extract_guid(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text[start..].find('}')? + start + 1;
    let guid = &text[start..end];

    if guid.len() == 38 && guid.contains('-') {
        Some(guid.to_string())
    } else {
        None
    }
}

pub fn remove_bcd_entry(guid: &str) -> Result<(), InstallerError> {
    println!("[BOOT] BCD giriÅŸi kaldÄ±rÄ±lÄ±yor: {}", guid);
    run_bcdedit(&["/delete", guid, "/f"])?;
    println!("[BOOT] BCD giriÅŸi kaldÄ±rÄ±ldÄ±: {}", guid);
    Ok(())
}

pub fn cleanup_esp(esp_letter: &str) -> Result<(), InstallerError> {
    let grub_dir = format!("{}:\\EFI\\NextOS", esp_letter);
    if std::path::Path::new(&grub_dir).exists() {
        let ps_script = format!(
            "Remove-Item -Path '{}' -Recurse -Force -ErrorAction Stop",
            grub_dir
        );
        run_powershell(&ps_script).map_err(|e| {
            InstallerError::BootloaderConfig(format!(
                "ESP temizleme hatası ({}): {}", grub_dir, e
            ))
        })?;
        println!("[BOOT] ESP temizlendi: {}", grub_dir);
    }
    Ok(())
}

/// Eski "Next OS Installer" / "NextOS" BCD boot kayıtlarını bulup siler.
/// bcdedit /enum firmware çıktısını parse ederek eşleşen GUID'leri bulur.
pub fn cleanup_old_boot_entries() -> Result<Vec<String>, InstallerError> {
    println!("[BOOT] Eski boot kayıtları taranıyor...");

    let ps_script = r#"
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
                if ($currentId -and ($desc -like '*Next OS*' -or $desc -like '*NextOS*')) {
                    $entries += "$currentId|$desc"
                }
            }
        }
        if ($entries.Count -eq 0) { "NONE" }
        else { $entries -join ";" }
    "#;

    let output = run_powershell(ps_script).map_err(|e| {
        InstallerError::BootloaderConfig(format!("BCD tarama hatası: {}", e))
    })?;

    let trimmed = output.trim();
    if trimmed == "NONE" || trimmed.is_empty() {
        println!("[BOOT] Eski boot kayıtları bulunamadı.");
        return Ok(vec![]);
    }

    let mut deleted = Vec::new();
    for entry in trimmed.split(';') {
        let parts: Vec<&str> = entry.splitn(2, '|').collect();
        if parts.len() < 2 {
            continue;
        }
        let guid = parts[0].trim();
        let desc = parts[1].trim();

        // {bootmgr} veya {fwbootmgr} gibi sistem kayıtlarını silme
        if guid == "{bootmgr}" || guid == "{fwbootmgr}" {
            println!("[BOOT] Sistem kaydı atlandı: {} ({})", guid, desc);
            continue;
        }

        println!("[BOOT] Eski kayıt siliniyor: {} ({})", guid, desc);
        match run_bcdedit(&["/delete", guid, "/f"]) {
            Ok(_) => {
                println!("[BOOT] Silindi: {} ({})", guid, desc);
                deleted.push(format!("{} ({})", guid, desc));
            }
            Err(e) => {
                println!("[BOOT] Silinemedi: {} ({}) - Hata: {}", guid, desc, e);
            }
        }
    }

    println!("[BOOT] Temizleme tamamlandı. {} kayıt silindi.", deleted.len());
    Ok(deleted)
}

pub fn reboot_system() -> Result<(), InstallerError> {
    println!("[BOOT] Sistem 5 saniye iÃ§inde yeniden baÅŸlatÄ±lacak...");

    let output = Command::new("shutdown")
        .args(["/r", "/t", "5", "/c", "Next OS Installer: Sistem yeniden baslatiliyor..."])
        .output()
        .map_err(|e| {
            InstallerError::BootloaderConfig(format!(
                "Yeniden baÅŸlatma komutu Ã§alÄ±ÅŸtÄ±rÄ±lamadÄ±: {}", e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::BootloaderConfig(format!(
            "Yeniden baÅŸlatma hatasÄ±: {}", stderr.trim()
        )));
    }

    Ok(())
}
