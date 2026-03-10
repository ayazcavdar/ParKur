// =============================================================================
// Next OS Installer - Ana Modül (lib.rs)
// =============================================================================
// Bu dosya tüm modülleri birleştirir ve Tauri komutlarını tanımlar.
// Frontend (HTML/JS) bu komutları `invoke()` ile çağırır.
//
// Mimari:
// ┌──────────────┐     ┌──────────────┐
// │   Frontend   │────>│  Tauri Cmd   │
// │  (HTML/JS)   │<────│  (lib.rs)    │
// └──────────────┘     └──────┬───────┘
//                             │
//                    ┌────────┼────────┐
//                    ▼        ▼        ▼
//               disk_ops  iso_ops  boot_ops
// =============================================================================

mod boot_ops;
mod disk_ops;
mod error;
mod iso_ops;

use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use tauri::Emitter; // Event yayınlama trait'i
use tauri::Manager; // Window/DevTools erişimi

// ─── Progress Event Payloads ────────────────────────────────────────────────

/// Frontend'e gönderilen ilerleme bilgisi
#[derive(Clone, Serialize, Deserialize)]
struct ProgressPayload {
    /// Hangi adımda olduğumuz (disk_shrink, iso_extract, bootloader, vb.)
    step: String,
    /// İlerleme yüzdesi (0-100)
    progress: u32,
    /// Kullanıcıya gösterilecek durum mesajı
    message: String,
}

// ─── Tauri Komutları ────────────────────────────────────────────────────────

/// Yönetici yetkisi kontrolü.
/// diskpart ve bcdedit için ZORUNLUDUR.
#[tauri::command]
async fn check_admin() -> Result<bool, InstallerError> {
    disk_ops::check_admin_privileges()
}

/// Sistemdeki tüm diskleri ve bölümlerini getirir.
/// Frontend disk seçim ekranında kullanır.
#[tauri::command]
async fn get_disk_info() -> Result<Vec<disk_ops::DiskInfo>, InstallerError> {
    disk_ops::get_disk_info()
}

/// Bir bölümün maksimum küçültülebilir alanını MB cinsinden döndürür.
/// Kullanıcının slider/input değerini sınırlamak için kullanılır.
#[tauri::command]
async fn get_max_shrink_size(drive_letter: String) -> Result<u64, InstallerError> {
    disk_ops::get_max_shrink_size_mb(&drive_letter)
}

/// Boot modunu tespit eder (UEFI veya Legacy BIOS).
#[tauri::command]
async fn detect_boot_mode() -> Result<boot_ops::BootMode, InstallerError> {
    boot_ops::detect_boot_mode()
}

/// Disk hazırlama: Seçilen bölümü küçültür ve yeni bölüm oluşturur.
///
/// Bu uzun süren bir işlemdir. İlerleme olayları (events) yayınlanır:
/// - Event: "installation-progress"
/// - Payload: { step, progress, message }
#[tauri::command]
async fn prepare_disk(
    app: tauri::AppHandle,
    disk_number: u32,
    partition_number: u32,
    shrink_size_mb: u64,
) -> Result<disk_ops::NewPartitionResult, InstallerError> {
    // ── Adım 1: Yönetici yetkisi kontrolü ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "disk_prepare".into(),
            progress: 0,
            message: "Yönetici yetkileri kontrol ediliyor...".into(),
        },
    );

    let is_admin = disk_ops::check_admin_privileges()?;
    if !is_admin {
        return Err(InstallerError::PermissionDenied(
            "Disk işlemleri için uygulamayı Yönetici (Administrator) olarak çalıştırın.".into(),
        ));
    }

    // ── Adım 2: Bölüm küçültme ve yeni bölüm oluşturma ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "disk_prepare".into(),
            progress: 10,
            message: format!(
                "Disk {} Bölüm {} küçültülüyor ({} MB)...",
                disk_number, partition_number, shrink_size_mb
            ),
        },
    );

    let result =
        disk_ops::shrink_and_create_partition(disk_number, partition_number, shrink_size_mb)?;

    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "disk_prepare".into(),
            progress: 100,
            message: format!(
                "Disk hazırlandı! Yeni bölüm: {}:\\ ({} MB, {})",
                result.drive_letter, result.size_mb, result.label
            ),
        },
    );

    Ok(result)
}

/// ISO çıkartma: ISO dosyasını monte eder, içeriğini hedef bölüme kopyalar.
///
/// İşlem Sırası:
/// 1. ISO'yu Windows'a monte et (Mount-DiskImage)
/// 2. İçeriği robocopy ile hedef bölüme kopyala
/// 3. Linux çekirdek dosyalarını (vmlinuz/initrd) bul
/// 4. ISO'yu demonte et
#[tauri::command]
async fn extract_iso(
    app: tauri::AppHandle,
    iso_path: String,
    target_drive_letter: String,
) -> Result<iso_ops::LinuxKernelInfo, InstallerError> {
    // ── Adım 1: ISO'yu monte et ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "iso_extract".into(),
            progress: 5,
            message: "ISO dosyası monte ediliyor...".into(),
        },
    );

    let iso_drive = iso_ops::mount_iso(&iso_path)?;

    // ── Adım 2: Dosyaları kopyala ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "iso_extract".into(),
            progress: 15,
            message: format!(
                "ISO içeriği kopyalanıyor ({}:\\ -> {}:\\)... Bu işlem birkaç dakika sürebilir.",
                iso_drive, target_drive_letter
            ),
        },
    );

    let copy_result = iso_ops::copy_iso_contents(&iso_drive, &target_drive_letter);

    // Kopyalama başarılı veya başarısız, ISO'yu demonte et
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "iso_extract".into(),
            progress: 85,
            message: "ISO demonte ediliyor...".into(),
        },
    );

    let _ = iso_ops::unmount_iso(&iso_path);

    copy_result?;

    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "iso_extract".into(),
            progress: 87,
            message: "ISO dosyası hedefe kopyalanıyor...".into(),
        },
    );

    iso_ops::copy_iso_file(&iso_path, &target_drive_letter)?;

    // ── Adım 3: Linux çekirdek dosyalarını bul ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "iso_extract".into(),
            progress: 90,
            message: "Linux çekirdek dosyaları aranıyor...".into(),
        },
    );

    let target_root = format!("{}:\\", target_drive_letter);
    let kernel_info = iso_ops::find_linux_kernel(&target_root)?;

    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "iso_extract".into(),
            progress: 100,
            message: format!(
                "ISO çıkartma tamamlandı! Çekirdek: {}, Initrd: {}",
                kernel_info.kernel_path, kernel_info.initrd_path
            ),
        },
    );

    Ok(kernel_info)
}

/// Bootloader yapılandırma: BCD'ye yeni boot girişi ekler.
///
/// UEFI modunda:
/// 1. ESP'yi monte et
/// 2. GRUB EFI dosyalarını ESP'ye kopyala
/// 3. grub.cfg oluştur
/// 4. bcdedit ile boot girişi oluştur
/// 5. Bir sonraki açılışta Linux'tan boot edilecek şekilde ayarla
#[tauri::command]
async fn configure_bootloader(
    app: tauri::AppHandle,
    iso_partition_letter: String,
    kernel_path: String,
    initrd_path: String,
) -> Result<boot_ops::BootConfigResult, InstallerError> {
    // ── Adım 1: Boot modunu tespit et ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "bootloader".into(),
            progress: 5,
            message: "Boot modu tespit ediliyor...".into(),
        },
    );

    let boot_mode = boot_ops::detect_boot_mode()?;

    if boot_mode == boot_ops::BootMode::LegacyBIOS {
        return Err(InstallerError::BootloaderConfig(
            "Legacy BIOS modu henüz desteklenmiyor. \
             Lütfen sisteminizin UEFI modunda olduğundan emin olun."
                .into(),
        ));
    }

    // ── Adım 2: ESP'yi monte et ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "bootloader".into(),
            progress: 15,
            message: "EFI System Partition monte ediliyor...".into(),
        },
    );

    let esp_letter = boot_ops::mount_esp()?;

    // ── Adım 3: GRUB kurulumu ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "bootloader".into(),
            progress: 30,
            message: "GRUB bootloader kuruluyor...".into(),
        },
    );

    let kernel_info = iso_ops::LinuxKernelInfo {
        kernel_path,
        initrd_path,
    };

    // GRUB kurulumu başarısız olursa temizleme yap
    if let Err(e) = boot_ops::setup_grub_efi(&iso_partition_letter, &esp_letter, &kernel_info) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    // ── Adım 4: BCD yapılandırması ──
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "bootloader".into(),
            progress: 60,
            message: "Windows Boot Manager yapılandırılıyor (bcdedit)...".into(),
        },
    );

    let bcd_guid = match boot_ops::create_bcd_entry(&esp_letter) {
        Ok(guid) => guid,
        Err(e) => {
            // BCD hatası durumunda ESP'yi temizle
            let _ = boot_ops::cleanup_esp(&esp_letter);
            return Err(e);
        }
    };

    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: "bootloader".into(),
            progress: 100,
            message: "Bootloader yapılandırması tamamlandı! Sistem yeniden başlatmaya hazır."
                .into(),
        },
    );

    Ok(boot_ops::BootConfigResult {
        boot_mode,
        bcd_entry_guid: bcd_guid,
        esp_path: Some(format!("{}:\\", esp_letter)),
        message: "Bootloader başarıyla yapılandırıldı.".into(),
    })
}

/// Sistemi yeniden başlatır (5 saniye gecikme ile).
#[tauri::command]
async fn reboot_system() -> Result<(), InstallerError> {
    boot_ops::reboot_system()
}

/// Temizleme: Hata durumunda BCD girişini ve ESP dosyalarını kaldırır.
#[tauri::command]
async fn cleanup_boot_entry(
    bcd_guid: String,
    esp_letter: String,
) -> Result<(), InstallerError> {
    let _ = boot_ops::remove_bcd_entry(&bcd_guid);
    let _ = boot_ops::cleanup_esp(&esp_letter);
    Ok(())
}

/// Eski "Next OS Installer" / "NextOS" BCD boot kayıtlarını temizler.
/// Frontend'deki "Eski Kayıtları Temizle" butonu bu komutu çağırır.
#[tauri::command]
async fn cleanup_old_boot_entries() -> Result<Vec<String>, InstallerError> {
    boot_ops::cleanup_old_boot_entries()
}

// ─── Uygulama Başlatma ─────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Debug build'lerde DevTools otomatik açılsın — siyah ekran teşhisi için
            #[cfg(debug_assertions)]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_admin,
            get_disk_info,
            get_max_shrink_size,
            detect_boot_mode,
            prepare_disk,
            extract_iso,
            configure_bootloader,
            reboot_system,
            cleanup_boot_entry,
            cleanup_old_boot_entries,
        ])
        .run(tauri::generate_context!())
        .expect("Next OS Installer başlatılamadı!");
}