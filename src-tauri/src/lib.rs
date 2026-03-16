mod boot_ops;
mod disk_ops;
mod error;
mod iso_ops;

use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use tauri::Emitter;
#[cfg(debug_assertions)]
use tauri::Manager;

#[derive(Clone, Serialize, Deserialize)]
struct ProgressPayload {
    step: String,
    progress: u32,
    message: String,
}

/// Yönetici yetkisi kontrolü.
#[tauri::command]
async fn check_admin() -> Result<bool, InstallerError> {
    disk_ops::check_admin_privileges()
}

/// Boot modunu tespit eder.
#[tauri::command]
async fn detect_boot_mode() -> Result<boot_ops::BootMode, InstallerError> {
    boot_ops::detect_boot_mode()
}

/// Eski boot kayıtlarını temizler.
#[tauri::command]
async fn cleanup_old_boot_entries() -> Result<Vec<String>, InstallerError> {
    boot_ops::cleanup_old_boot_entries()
}

/// Disk bölümlerini listeler.
#[tauri::command]
async fn get_disk_partitions() -> Result<Vec<disk_ops::PartitionInfo>, InstallerError> {
    disk_ops::list_partitions()
}

/// Tek komutla tüm kurulumu gerçekleştirir:
/// disk hazırlama, ISO kopyalama, bootloader yapılandırma, otomatik yeniden başlatma.
#[tauri::command]
async fn start_installation(
    app: tauri::AppHandle,
    iso_path: String,
    disk_number: u32,
    partition_number: u32,
    part_letter: String,
    shrink_gb: u32,
) -> Result<(), InstallerError> {
    // 1. Yönetici yetkisi kontrolü
    emit_progress(&app, "check", 0, "Yönetici yetkileri kontrol ediliyor...");

    if !disk_ops::check_admin_privileges()? {
        return Err(InstallerError::PermissionDenied(
            "Uygulamayı Yönetici (Administrator) olarak çalıştırın.".into(),
        ));
    }

    // 2. Boot modu kontrolü
    emit_progress(&app, "check", 5, "Boot modu kontrol ediliyor...");

    let boot_mode = boot_ops::detect_boot_mode()?;
    if boot_mode == boot_ops::BootMode::LegacyBIOS {
        return Err(InstallerError::BootloaderConfig(
            "Legacy BIOS desteklenmiyor. Sistem UEFI modunda olmalı.".into(),
        ));
    }

    // 3. Kullanıcının seçtiği boyutu kullan
    emit_progress(&app, "disk", 10, "Disk boyutu ayarlanıyor...");

    let shrink_mb: u64 = (shrink_gb as u64) * 1024;

    // 4. Seçilen bölümü kullan
    emit_progress(
        &app,
        "disk",
        20,
        &format!(
            "Seçilen bölüm: {}:\\ — {} MB ayrılacak",
            part_letter, shrink_mb
        ),
    );

    // 5. Bölüm küçült + yeni bölüm oluştur
    emit_progress(&app, "disk", 25, "Disk bölümü küçültülüyor...");

    let new_part = disk_ops::shrink_and_create_partition(disk_number, partition_number, shrink_mb)?;

    emit_progress(
        &app,
        "disk",
        35,
        &format!("Yeni bölüm oluşturuldu: {}:\\", new_part.drive_letter),
    );

    // 6. ISO'yu monte et (çekirdek + GRUB EFI dosyaları için)
    emit_progress(&app, "iso", 40, "ISO dosyası monte ediliyor...");

    let iso_drive = iso_ops::mount_iso(&iso_path)?;

    // 7. Linux çekirdek dosyalarını bul
    emit_progress(&app, "iso", 45, "Linux çekirdek dosyaları aranıyor...");

    let kernel_info = match iso_ops::find_linux_kernel(&iso_drive) {
        Ok(ki) => ki,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };

    emit_progress(
        &app,
        "iso",
        50,
        &format!("Çekirdek bulundu: {}", kernel_info.kernel_path),
    );

    // 8. ESP monte et + GRUB kur
    emit_progress(&app, "boot", 55, "EFI System Partition monte ediliyor...");

    let esp_letter = match boot_ops::mount_esp() {
        Ok(l) => l,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };

    emit_progress(&app, "boot", 60, "GRUB bootloader kuruluyor...");

    if let Err(e) = boot_ops::setup_grub_efi(&iso_drive, &esp_letter, &kernel_info) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    // 9. ISO'yu demonte et
    let _ = iso_ops::unmount_iso(&iso_path);

    // 10. ISO dosyasını yeni bölüme kopyala
    emit_progress(
        &app,
        "iso",
        65,
        "ISO dosyası kopyalanıyor (bu birkaç dakika sürebilir)...",
    );

    if let Err(e) = iso_ops::copy_iso_to_partition(&iso_path, &new_part.drive_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    emit_progress(&app, "iso", 85, "ISO dosyası kopyalandı.");

    // 10.5. Data bölümüne grub.cfg yaz
    emit_progress(&app, "boot", 87, "Data bölümüne GRUB yapılandırması yazılıyor...");
    let _ = boot_ops::write_grub_cfg_to_data_partition(&new_part.drive_letter, &kernel_info);

    // 11. BCD yapılandırması
    emit_progress(&app, "boot", 90, "Windows Boot Manager yapılandırılıyor...");

    if let Err(e) = boot_ops::create_bcd_entry(&esp_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    emit_progress(&app, "boot", 95, "Bootloader yapılandırması tamamlandı.");

    // 12. Otomatik yeniden başlatma
    emit_progress(
        &app,
        "reboot",
        100,
        "Sistem yeniden başlatılıyor...",
    );

    std::thread::sleep(std::time::Duration::from_secs(2));
    boot_ops::reboot_system()?;

    Ok(())
}

fn emit_progress(app: &tauri::AppHandle, step: &str, progress: u32, message: &str) {
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: step.to_string(),
            progress,
            message: message.to_string(),
        },
    );
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|_app| {
            #[cfg(debug_assertions)]
            {
                if let Some(window) = _app.get_webview_window("main") {
                    window.open_devtools();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_admin,
            detect_boot_mode,
            start_installation,
            cleanup_old_boot_entries,
            get_disk_partitions,
        ])
        .run(tauri::generate_context!())
        .expect("Uygulama başlatılamadı!");
}
