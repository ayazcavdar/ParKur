mod boot_ops;
mod disk_ops;
mod error;
mod iso_ops;
mod preseed_ops;

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

/// ISO dosyasının boyutunu MB cinsinden döndürür.
#[tauri::command]
async fn get_iso_size_mb(path: String) -> Result<u64, InstallerError> {
    iso_ops::get_iso_size_mb(&path)
}

/// Kullanıcı girdisini doğrular.
fn validate_user_input(
    user_name: &str,
    password: &str,
) -> Result<(), InstallerError> {
    if user_name.is_empty() {
        return Err(InstallerError::InvalidInput(
            "Kullanıcı adı boş olamaz.".into(),
        ));
    }
    if password.is_empty() {
        return Err(InstallerError::InvalidInput("Şifre boş olamaz.".into()));
    }
    // Linux kullanıcı adı kuralları: küçük harfle başlamalı,
    // sadece küçük harf, rakam, tire ve alt çizgi içerebilir
    let valid_chars = user_name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    let starts_ok = user_name
        .chars()
        .next()
        .map(|c| c.is_ascii_lowercase())
        .unwrap_or(false);
    if !valid_chars || !starts_ok {
        return Err(InstallerError::InvalidInput(
            "Kullanıcı adı küçük harfle başlamalı, sadece küçük harf, rakam, tire ve alt çizgi içerebilir.".into(),
        ));
    }
    if user_name.len() > 32 {
        return Err(InstallerError::InvalidInput(
            "Kullanıcı adı 32 karakterden uzun olamaz.".into(),
        ));
    }
    Ok(())
}

/// Tek komutla tüm kurulumu gerçekleştirir:
/// disk hazırlama (dual partition), ISO kopyalama, preseed dosyaları,
/// bootloader yapılandırma, otomatik yeniden başlatma.
#[tauri::command]
async fn start_installation(
    app: tauri::AppHandle,
    iso_path: String,
    disk_number: u32,
    partition_number: u32,
    part_letter: String,
    shrink_gb: u32,
    user_name: String,
    password: String,
) -> Result<(), InstallerError> {
    // 1. Yönetici yetkisi kontrolü
    emit_progress(&app, "check", 0, "Yönetici yetkileri kontrol ediliyor...");

    if !disk_ops::check_admin_privileges()? {
        return Err(InstallerError::PermissionDenied(
            "Uygulamayı Yönetici (Administrator) olarak çalıştırın.".into(),
        ));
    }

    // 2. Boot modu kontrolü
    emit_progress(&app, "check", 3, "Boot modu kontrol ediliyor...");

    let boot_mode = boot_ops::detect_boot_mode()?;
    if boot_mode == boot_ops::BootMode::LegacyBIOS {
        return Err(InstallerError::BootloaderConfig(
            "Legacy BIOS desteklenmiyor. Sistem UEFI modunda olmalı.".into(),
        ));
    }

    // 3. Kullanıcı bilgisi doğrulama
    emit_progress(&app, "check", 5, "Kullanıcı bilgileri doğrulanıyor...");
    validate_user_input(&user_name, &password)?;

    // 4. ISO dosya boyutunu hesapla (disk matematiği için)
    emit_progress(&app, "disk", 8, "ISO dosya boyutu hesaplanıyor...");
    let iso_size_mb = iso_ops::get_iso_size_mb(&iso_path)?;
    let shrink_mb: u64 = (shrink_gb as u64) * 1024;
    let persistence_mb = iso_size_mb + 1024;

    emit_progress(
        &app,
        "disk",
        10,
        &format!(
            "Seçilen bölüm: {}:\\ — {} MB toplam, Persistence: {} MB (FAT32), Linux: {} MB",
            part_letter,
            shrink_mb,
            persistence_mb,
            shrink_mb.saturating_sub(persistence_mb + 50)
        ),
    );

    // 5. Bölüm küçült + dual partition oluştur (önce Persistence FAT32, sonra Linux)
    emit_progress(&app, "disk", 15, "Disk bölümleri oluşturuluyor (Persistence FAT32 + Linux)...");

    let dual_part = disk_ops::shrink_and_create_dual_partitions(
        disk_number,
        partition_number,
        shrink_mb,
        iso_size_mb,
    )?;

    emit_progress(
        &app,
        "disk",
        30,
        &format!(
            "Bölümler oluşturuldu: Persistence={}:\\ ({} MB FAT32), Linux=Bölüm {} ({} MB)",
            dual_part.persistence_letter,
            dual_part.persistence_mb,
            dual_part.linux_partition_number,
            dual_part.linux_mb
        ),
    );

    // 6. Race Condition Koruması: Windows'un diski bağlaması için 5 saniye bekle
    emit_progress(&app, "disk", 33, "Windows disk bağlama bekleniyor (5 saniye)...");
    std::thread::sleep(std::time::Duration::from_secs(5));

    // 7. ISO'yu monte et (çekirdek + GRUB EFI dosyaları için)
    emit_progress(&app, "iso", 38, "ISO dosyası monte ediliyor...");

    let iso_drive = iso_ops::mount_iso(&iso_path)?;

    // 8. Linux çekirdek dosyalarını bul
    emit_progress(&app, "iso", 42, "Linux çekirdek dosyaları aranıyor...");

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

    // 9. ESP monte et + GRUB kur
    emit_progress(&app, "boot", 48, "EFI System Partition monte ediliyor...");

    let esp_letter = match boot_ops::mount_esp() {
        Ok(l) => l,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };

    emit_progress(&app, "boot", 52, "GRUB bootloader kuruluyor...");

    if let Err(e) = boot_ops::setup_grub_efi(&iso_drive, &esp_letter, &kernel_info) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    // 10. ISO'yu demonte et
    let _ = iso_ops::unmount_iso(&iso_path);

    // 11. ISO dosyasını Persistence bölümüne kopyala (FAT32)
    emit_progress(
        &app,
        "iso",
        58,
        "ISO dosyası kopyalanıyor (bu birkaç dakika sürebilir)...",
    );

    if let Err(e) = iso_ops::copy_iso_to_partition(&iso_path, &dual_part.persistence_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    emit_progress(&app, "iso", 72, "ISO dosyası kopyalandı.");

    // 12. persistence.conf yaz (CRLF→LF koruması)
    emit_progress(&app, "boot", 75, "persistence.conf yazılıyor...");
    let persistence_conf = preseed_ops::generate_persistence_conf();
    let persistence_conf_path = format!(
        "{}:\\persistence.conf",
        dual_part.persistence_letter
    );
    preseed_ops::write_linux_file(&persistence_conf_path, &persistence_conf)?;

    // 13. install-hook.sh yaz (CRLF→LF koruması, kullanıcı bilgileri ile)
    emit_progress(&app, "boot", 78, "install-hook.sh yazılıyor (kullanıcı oluşturma betiği)...");
    let install_hook = preseed_ops::generate_install_hook(&user_name, &password);
    let install_hook_path = format!(
        "{}:\\install-hook.sh",
        dual_part.persistence_letter
    );
    preseed_ops::write_linux_file(&install_hook_path, &install_hook)?;

    // 14. Data bölümüne grub.cfg yaz (CRLF→LF koruması)
    emit_progress(&app, "boot", 82, "Data bölümüne GRUB yapılandırması yazılıyor...");
    let _ = boot_ops::write_grub_cfg_to_data_partition(&dual_part.persistence_letter, &kernel_info);

    // 15. BCD yapılandırması
    emit_progress(&app, "boot", 88, "Windows Boot Manager yapılandırılıyor...");

    if let Err(e) = boot_ops::create_bcd_entry(&esp_letter) {
        let _ = boot_ops::cleanup_esp(&esp_letter);
        return Err(e);
    }

    emit_progress(&app, "boot", 95, "Bootloader yapılandırması tamamlandı.");

    // 16. Otomatik yeniden başlatma
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
            get_iso_size_mb,
        ])
        .run(tauri::generate_context!())
        .expect("Uygulama başlatılamadı!");
}
