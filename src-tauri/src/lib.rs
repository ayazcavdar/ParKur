mod boot_ops;
mod config_ops;
mod cpio_ops;
mod disk_ops;
mod error;
mod iso_ops;

use crate::config_ops::UserConfig;
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

/// Kullanıcı yapılandırmasını doğrular.
#[tauri::command]
async fn validate_user_config(
    username: String,
    password: String,
    hostname: String,
) -> Result<(), InstallerError> {
    let config = UserConfig {
        username,
        password,
        hostname,
        locale: "tr_TR.UTF-8".into(),
        timezone: "Europe/Istanbul".into(),
        keyboard: "tr".into(),
    };
    config_ops::validate_user_config(&config)
}

/// Tek komutla tüm kurulumu gerçekleştirir:
/// disk hazırlama, ISO kopyalama, kullanıcı config yazma,
/// supplementary initrd oluşturma, bootloader yapılandırma, reboot.
#[tauri::command]
async fn start_installation(
    app: tauri::AppHandle,
    iso_path: String,
    disk_number: u32,
    partition_number: u32,
    part_letter: String,
    shrink_gb: u32,
    username: String,
    password: String,
    hostname: String,
    locale: String,
    timezone: String,
    keyboard: String,
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

    // 3. Kullanıcı yapılandırmasını doğrula
    emit_progress(&app, "check", 8, "Kullanıcı bilgileri doğrulanıyor...");

    let user_config = UserConfig {
        username,
        password,
        hostname,
        locale,
        timezone,
        keyboard,
    };
    config_ops::validate_user_config(&user_config)?;

    // 4. Disk boyutu hesapla ve bölüm oluştur
    emit_progress(&app, "disk", 10, "Disk boyutu ayarlanıyor...");
    let shrink_mb: u64 = (shrink_gb as u64) * 1024;

    emit_progress(
        &app,
        "disk",
        15,
        &format!(
            "Seçilen bölüm: {}:\\ — {} MB ayrılacak",
            part_letter, shrink_mb
        ),
    );

    emit_progress(&app, "disk", 20, "Disk bölümü küçültülüyor...");

    let new_part = disk_ops::shrink_and_create_partition(disk_number, partition_number, shrink_mb)?;

    emit_progress(
        &app,
        "disk",
        30,
        &format!("Yeni bölüm oluşturuldu: {}:\\", new_part.drive_letter),
    );

    // 5. ISO'yu monte et
    emit_progress(&app, "iso", 35, "ISO dosyası monte ediliyor...");

    let iso_drive = iso_ops::mount_iso(&iso_path)?;

    // 6. Linux çekirdek dosyalarını bul
    emit_progress(&app, "iso", 40, "Linux çekirdek dosyaları aranıyor...");

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
        45,
        &format!("Çekirdek bulundu: {}", kernel_info.kernel_path),
    );

    // 7. ESP monte et + GRUB kur
    emit_progress(&app, "boot", 48, "EFI System Partition monte ediliyor...");

    let esp_letter = match boot_ops::mount_esp() {
        Ok(l) => l,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };

    emit_progress(&app, "boot", 50, "GRUB bootloader kuruluyor...");

    if let Err(e) = boot_ops::setup_grub_efi(&iso_drive, &esp_letter, &kernel_info) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    // 8. ISO'yu demonte et
    let _ = iso_ops::unmount_iso(&iso_path);

    // 9. ISO dosyasını yeni bölüme kopyala
    emit_progress(
        &app,
        "iso",
        55,
        "ISO dosyası kopyalanıyor (bu birkaç dakika sürebilir)...",
    );

    if let Err(e) = iso_ops::copy_iso_to_partition(&iso_path, &new_part.drive_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    emit_progress(&app, "iso", 70, "ISO dosyası kopyalandı.");

    // 10. Kullanıcı yapılandırmasını diske yaz (parkur.conf)
    emit_progress(&app, "config", 73, "Kullanıcı yapılandırması yazılıyor...");

    if let Err(e) = config_ops::write_parkur_conf(&new_part.drive_letter, &user_config) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    // 11. Supplementary initrd oluştur ve diske yaz (parkur-hook.img)
    emit_progress(&app, "config", 78, "Kurulum motoru hazırlanıyor (initrd)...");

    if let Err(e) = cpio_ops::write_initrd_to_partition(&new_part.drive_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    // 12. Data bölümüne grub.cfg yaz (yedek)
    emit_progress(&app, "boot", 82, "Data bölümüne GRUB yapılandırması yazılıyor...");
    let _ = boot_ops::write_grub_cfg_to_data_partition(&new_part.drive_letter, &kernel_info);

    // 13. BCD yapılandırması
    emit_progress(&app, "boot", 88, "Windows Boot Manager yapılandırılıyor...");

    if let Err(e) = boot_ops::create_bcd_entry(&esp_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    emit_progress(&app, "boot", 95, "Bootloader yapılandırması tamamlandı.");

    // 14. Otomatik yeniden başlatma
    emit_progress(
        &app,
        "reboot",
        100,
        "Sistem yeniden başlatılıyor... Pardus otonom kurulum başlayacak.",
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
            validate_user_config,
        ])
        .run(tauri::generate_context!())
        .expect("Uygulama başlatılamadı!");
}
