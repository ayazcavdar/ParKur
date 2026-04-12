use crate::disk_ops::run_powershell;
use crate::error::InstallerError;
use crate::iso_ops::LinuxKernelInfo;
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum BootMode {
    UEFI,
    LegacyBIOS,
}

/// Sistem boot modunu tespit eder (UEFI veya Legacy BIOS).
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

/// EFI System Partition'ı monte eder, sürücü harfini döndürür.
pub fn mount_esp() -> Result<String, InstallerError> {
    let script = r#"
        $espPart = Get-Partition | Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' } | Select-Object -First 1
        if (-not $espPart) { throw "EFI System Partition bulunamadi." }

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
        if (-not $targetLetter) { throw "ESP icin uygun surucu harfi bulunamadi." }

        Add-PartitionAccessPath -DiskNumber $espPart.DiskNumber -PartitionNumber $espPart.PartitionNumber -AccessPath "${targetLetter}:\" -ErrorAction Stop
        Start-Sleep -Milliseconds 1000
        $targetLetter
    "#;

    let output = run_powershell(script).map_err(|e| {
        InstallerError::BootloaderConfig(format!("ESP monte edilemedi: {}", e))
    })?;

    let letter = output.trim().to_string();
    if letter.len() != 1 {
        return Err(InstallerError::BootloaderConfig(format!(
            "ESP sürücü harfi alınamadı: '{}'",
            letter
        )));
    }

    println!("[BOOT] ESP monte edildi: {}:\\", letter);
    Ok(letter)
}

/// ISO'daki GRUB EFI dosyalarını ESP'ye kopyalar ve grub.cfg oluşturur.
pub fn setup_grub_efi(
    iso_drive: &str,
    esp_letter: &str,
    kernel_info: &LinuxKernelInfo,
) -> Result<(), InstallerError> {
    let esp_grub_dir = format!("{}:\\EFI\\NextOS", esp_letter);
    let iso_root = format!("{}:\\", iso_drive);

    // ESP üzerinde dizin oluştur (PowerShell -Force: eksik üst dizinleri zorla oluşturur)
    let mkdir_script = format!(
        "New-Item -Path '{}' -ItemType Directory -Force -ErrorAction SilentlyContinue | Out-Null",
        esp_grub_dir
    );
    let _ = run_powershell(&mkdir_script);

    // ISO'dan GRUB EFI binary bul
    let grub_candidates = [
        format!("{}EFI\\boot\\grubx64.efi", iso_root),
        format!("{}EFI\\Boot\\grubx64.efi", iso_root),
        format!("{}EFI\\BOOT\\GRUBX64.EFI", iso_root),
        format!("{}EFI\\BOOT\\grubx64.efi", iso_root),
        format!("{}EFI\\boot\\bootx64.efi", iso_root),
        format!("{}EFI\\Boot\\bootx64.efi", iso_root),
        format!("{}EFI\\BOOT\\BOOTX64.EFI", iso_root),
        format!("{}EFI\\BOOT\\bootx64.efi", iso_root),
    ];

    let grub_source = grub_candidates
        .iter()
        .find(|p| {
            let path = std::path::Path::new(p.as_str());
            if !path.exists() {
                return false;
            }
            match std::fs::read(path) {
                Ok(data) => {
                    data.len() > 10_000
                        && data.get(0) == Some(&0x4D)
                        && data.get(1) == Some(&0x5A)
                }
                Err(_) => false,
            }
        })
        .ok_or_else(|| {
            InstallerError::BootloaderConfig(
                "ISO içinde geçerli bir GRUB EFI dosyası bulunamadı. \
                 64-bit UEFI destekli bir ISO kullandığınızdan emin olun."
                    .into(),
            )
        })?;

    // GRUB EFI'yi ESP'ye kopyala (PowerShell -Force: EFI ACL kısıtlamasını aşar)
    let grub_dest = format!("{}\\grubx64.efi", esp_grub_dir);
    let copy_ps = format!(
        "Copy-Item -Path '{}' -Destination '{}' -Force -ErrorAction Stop",
        grub_source.replace("'", "''"),
        grub_dest.replace("'", "''")
    );
    run_powershell(&copy_ps).map_err(|e| {
        InstallerError::BootloaderConfig(format!("GRUB EFI kopyalanamadı: {}", e))
    })?;

    println!("[BOOT] GRUB EFI kopyalandı: {} -> {}", grub_source, grub_dest);

    // GRUB modüllerini kopyala (varsa)
    // ISO'daki olası GRUB modül konumlarını dene
    let grub_module_candidates = [
        format!("{}boot\\grub", iso_root),
        format!("{}boot\\grub\\x86_64-efi", iso_root),
        format!("{}EFI\\boot\\grub", iso_root),
        format!("{}EFI\\debian\\grub", iso_root),
    ];

    let grub_modules_src = grub_module_candidates
        .iter()
        .find(|p| std::path::Path::new(p.as_str()).is_dir())
        .cloned()
        .unwrap_or_else(|| format!("{}boot\\grub", iso_root));

    if std::path::Path::new(&grub_modules_src).is_dir() {
        // grubx64.efi derleme prefix'i bilinmediğinden tüm olası hedeflere kopyala
        let all_module_dests = [
            format!("{}\\grub", esp_grub_dir),               // /EFI/NextOS/grub
            format!("{}:\\boot\\grub", esp_letter),           // /boot/grub (en yaygın)
            format!("{}:\\EFI\\debian\\grub", esp_letter),    // Debian prefix
            format!("{}:\\EFI\\debian", esp_letter),          // Debian flat
            format!("{}:\\EFI\\pardus\\grub", esp_letter),    // Pardus prefix
            format!("{}:\\grub", esp_letter),                 // root grub
        ];

        for dest in &all_module_dests {
            let _ = Command::new("robocopy")
                .args([
                    &grub_modules_src,
                    dest,
                    "/E",
                    "/R:2",
                    "/W:1",
                    "/NFL",
                    "/NDL",
                    "/NJH",
                    "/NJS",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
        println!("[BOOT] GRUB modülleri tüm prefix konumlarına kopyalandı");
    } else {
        println!("[BOOT] UYARI: ISO'da GRUB modül dizini bulunamadı, insmod komutları çalışmayabilir");
    }

    let grub_cfg = generate_grub_cfg(kernel_info);

    // grub.cfg'yi tüm olası GRUB prefix konumlarına yaz (CRLF→LF, BOM'suz)
    let cfg_locations = [
        format!("{}\\grub.cfg", esp_grub_dir),
        format!("{}\\grub\\grub.cfg", esp_grub_dir),
        format!("{}\\boot\\grub\\grub.cfg", esp_grub_dir),
        format!("{}:\\EFI\\debian\\grub.cfg", esp_letter),
        format!("{}:\\EFI\\pardus\\grub.cfg", esp_letter),
        format!("{}:\\EFI\\BOOT\\grub.cfg", esp_letter),
        format!("{}:\\boot\\grub\\grub.cfg", esp_letter),
        format!("{}:\\grub\\grub.cfg", esp_letter),
    ];

    for cfg_path in &cfg_locations {
        write_linux_config_file(cfg_path, &grub_cfg).map_err(|e| {
            InstallerError::BootloaderConfig(format!("grub.cfg yazılamadı ({}): {}", cfg_path, e))
        })?;
    }

    println!("[BOOT] GRUB kurulumu tamamlandı");
    Ok(())
}

/// Linux hedef dosyasını CRLF → LF dönüşümü ile diske yazar. BOM eklenmez.
/// EFI bölümüne doğrudan std::fs::write yapılamaz (OS Error 5).
/// Çözüm: temp dizine yaz → PowerShell New-Item -Force + Copy-Item -Force ile hedef yola taşı.
fn write_linux_config_file(path: &str, content: &str) -> Result<(), InstallerError> {
    let lf_content = content.replace("\r\n", "\n");

    // 1. Geçici dosyaya yaz (temp dizine std::fs::write sorunsuz çalışır)
    let temp_file = std::env::temp_dir().join(format!(
        "nextos_cfg_{}.tmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::write(&temp_file, lf_content.as_bytes()).map_err(|e| {
        InstallerError::BootloaderConfig(format!(
            "Geçici dosya yazılamadı ({}): {}",
            temp_file.display(),
            e
        ))
    })?;

    // 2. PowerShell ile üst dizini zorla oluştur + dosyayı kopyala
    //    -Force: eksik klasör ağacını oluşturur ve ACL kısıtlamalarını aşar
    let parent_dir = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let ps_script = format!(
        "New-Item -Path '{}' -ItemType Directory -Force -ErrorAction SilentlyContinue | Out-Null; Copy-Item -Path '{}' -Destination '{}' -Force -ErrorAction Stop",
        parent_dir.replace("'", "''"),
        temp_file.to_string_lossy().replace("'", "''"),
        path.replace("'", "''")
    );

    let result = run_powershell(&ps_script);

    // Geçici dosyayı temizle
    let _ = std::fs::remove_file(&temp_file);

    result.map_err(|e| {
        InstallerError::BootloaderConfig(format!(
            "Dosya kopyalanamadı ({}): {}",
            path, e
        ))
    })?;

    Ok(())
}

/// GRUB grub.cfg içeriğini oluşturur.
/// timeout=0: Menüde bekleme yok (mavi ekran bypass).
/// text systemd.unit=multi-user.target: TTY hedefi (siyah ekran, log görülebilir).
/// insmod fat: Persistence bölümü FAT32 (live-boot NTFS okuyamaz).
///
/// Kritik: grubx64.efi'nin derleme zamanı prefix'i bilinmediği için,
/// grub.cfg'de hem ESP prefix'leri hem de ISO prefix'leri denenir.
/// search --file /install.iso ile arama yapılır (label'a bağımlı değil).
pub fn generate_grub_cfg(kernel_info: &LinuxKernelInfo) -> String {
    let boot_keyword = if kernel_info.kernel_path.starts_with("casper/") {
        "boot=casper"
    } else {
        "boot=live"
    };

    let cfg = format!(
        r#"# GRUB prefix ayarları — modülleri bulabilmesi için birden fazla konum dene
if [ -e $prefix/x86_64-efi/normal.mod ]; then
    true
elif [ -e /EFI/NextOS/grub/x86_64-efi/normal.mod ]; then
    set prefix=($root)/EFI/NextOS/grub
elif [ -e /boot/grub/x86_64-efi/normal.mod ]; then
    set prefix=($root)/boot/grub
elif [ -e /EFI/debian/grub/x86_64-efi/normal.mod ]; then
    set prefix=($root)/EFI/debian/grub
fi

set default="0"
set timeout="0"
set timeout_style="hidden"

insmod part_gpt
insmod part_msdos
insmod fat
insmod search
insmod search_label
insmod search_fs_file
insmod all_video
insmod gfxterm
insmod loopback
insmod iso9660

menuentry "Pardus Live" {{
    set isofile="/install.iso"
    search --no-floppy --file $isofile --set root
    loopback loop $isofile
    linux (loop)/{kernel} {boot_kw} findiso=$isofile components quiet splash locales=tr_TR.UTF-8 keyboard-layouts=tr timezone=Europe/Istanbul text systemd.unit=multi-user.target
    initrd (loop)/{initrd}
}}"#,
        kernel = kernel_info.kernel_path.replace('\\', "/"),
        initrd = kernel_info.initrd_path.replace('\\', "/"),
        boot_kw = boot_keyword,
    );

    // CRLF → LF koruması
    cfg.replace("\r\n", "\n")
}

/// Data bölümüne grub.cfg yazar (GRUB'ın veri bölümünde de config bulabilmesi için).
pub fn write_grub_cfg_to_data_partition(
    data_letter: &str,
    kernel_info: &LinuxKernelInfo,
) -> Result<(), InstallerError> {
    let grub_cfg = generate_grub_cfg(kernel_info);

    // CRLF→LF dönüşümü ile grub.cfg yaz
    let cfg_locations = [
        format!("{}:\\boot\\grub\\grub.cfg", data_letter),
        format!("{}:\\grub\\grub.cfg", data_letter),
        format!("{}:\\EFI\\BOOT\\grub.cfg", data_letter),
    ];

    for cfg_path in &cfg_locations {
        let _ = write_linux_config_file(cfg_path, &grub_cfg);
    }

    println!("[BOOT] Data partition grub.cfg yazıldı: {}:\\", data_letter);
    Ok(())
}

/// BCD'ye firmware boot girişi ekler ve bootsequence ayarlar.
/// bootsequence tek seferlik boot, Windows Boot Manager menüsü görünmez.
pub fn create_bcd_entry(esp_letter: &str) -> Result<String, InstallerError> {
    // Eski NextOS kayıtlarını temizle
    let _ = cleanup_old_boot_entries();

    // Yeni firmware boot girişi oluştur
    let output = Command::new("bcdedit")
        .args(["/copy", "{bootmgr}", "/d", "NextOS Installer"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| {
            InstallerError::BootloaderConfig(format!("bcdedit başlatılamadı: {}", e))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::BootloaderConfig(format!(
            "bcdedit hatası: {} {}",
            stdout.trim(),
            stderr.trim()
        )));
    }

    let guid = extract_guid(&stdout).ok_or_else(|| {
        InstallerError::BootloaderConfig(format!(
            "BCD GUID ayrıştırılamadı: '{}'",
            stdout.trim()
        ))
    })?;

    println!("[BOOT] BCD girişi oluşturuldu: {}", guid);

    // Girişi yapılandır
    run_bcdedit(&["/set", &guid, "device", &format!("partition={}:", esp_letter)])?;
    run_bcdedit(&["/set", &guid, "path", "\\EFI\\NextOS\\grubx64.efi"])?;
    run_bcdedit(&["/set", &guid, "description", "NextOS Installer"])?;
    run_bcdedit(&[
        "/set",
        "{fwbootmgr}",
        "displayorder",
        &guid,
        "/addfirst",
    ])?;

    // bootsequence: Tek seferlik boot — menü gösterilmez
    run_bcdedit(&["/set", "{fwbootmgr}", "bootsequence", &guid])?;

    println!("[BOOT] BCD yapılandırması tamamlandı: {}", guid);
    Ok(guid)
}

fn run_bcdedit(args: &[&str]) -> Result<String, InstallerError> {
    let output = Command::new("bcdedit")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| {
            InstallerError::BootloaderConfig(format!("bcdedit çalıştırılamadı: {}", e))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::BootloaderConfig(format!(
            "bcdedit {:?} hatası: {} {}",
            args,
            stdout.trim(),
            stderr.trim()
        )));
    }

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

/// Eski NextOS BCD boot kayıtlarını temizler.
pub fn cleanup_old_boot_entries() -> Result<Vec<String>, InstallerError> {
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
                if ($currentId -and ($desc -like '*Next*OS*' -or $desc -like '*NextOS*')) {
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
            println!("[BOOT] Eski kayıt silindi: {}", guid);
        }
    }

    Ok(deleted)
}

/// ESP üzerindeki NextOS dosyalarını temizler.
pub fn cleanup_esp(esp_letter: &str) -> Result<(), InstallerError> {
    let grub_dir = format!("{}:\\EFI\\NextOS", esp_letter);
    if std::path::Path::new(&grub_dir).exists() {
        let script = format!(
            "Remove-Item -Path '{}' -Recurse -Force -ErrorAction SilentlyContinue",
            grub_dir
        );
        let _ = run_powershell(&script);
        println!("[BOOT] ESP temizlendi: {}", grub_dir);
    }
    Ok(())
}

/// Sistemi yeniden başlatır (3 saniye gecikme).
pub fn reboot_system() -> Result<(), InstallerError> {
    println!("[BOOT] Sistem 3 saniye içinde yeniden başlatılacak...");

    let output = Command::new("shutdown")
        .args(["/r", "/t", "3", "/c", "Pardus Live baslatiliyor..."])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| {
            InstallerError::BootloaderConfig(format!("Yeniden başlatma hatası: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::BootloaderConfig(format!(
            "Yeniden başlatma hatası: {}",
            stderr.trim()
        )));
    }

    Ok(())
}
