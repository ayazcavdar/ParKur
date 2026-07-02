mod boot_ops;
mod disk_ops;
mod error;
mod image_ops;
mod initramfs_ops;
mod iso_ops;
mod util;

use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use tauri::Emitter;
#[cfg(debug_assertions)]
use tauri::Manager;

const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct ProgressPayload {
    step: String,
    progress: u32,
    message: String,
}

#[tauri::command]
async fn check_admin() -> Result<bool, InstallerError> {
    disk_ops::check_admin_privileges()
}

#[tauri::command]
async fn detect_boot_mode() -> Result<boot_ops::BootMode, InstallerError> {
    boot_ops::detect_boot_mode()
}

#[tauri::command]
async fn cleanup_old_boot_entries() -> Result<Vec<String>, InstallerError> {
    boot_ops::cleanup_nextos_firmware_entries()
}

#[tauri::command]
async fn list_host_partitions() -> Result<Vec<disk_ops::HostCandidate>, InstallerError> {
    disk_ops::list_host_candidates()
}

#[tauri::command]
async fn get_iso_size_mb(path: String) -> Result<u64, InstallerError> {
    iso_ops::get_iso_size_mb(&path)
}

fn validate_user_input(
    user_name: &str,
    password: &str,
    hostname: &str,
) -> Result<(), InstallerError> {
    if user_name.is_empty() {
        return Err(InstallerError::InvalidInput("Username cannot be empty.".into()));
    }
    if password.is_empty() {
        return Err(InstallerError::InvalidInput("Password cannot be empty.".into()));
    }
    if hostname.is_empty() {
        return Err(InstallerError::InvalidInput("Hostname cannot be empty.".into()));
    }
    let valid_user = user_name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    let starts_user_ok = user_name
        .chars()
        .next()
        .map(|c| c.is_ascii_lowercase())
        .unwrap_or(false);
    if !valid_user || !starts_user_ok || user_name.len() > 32 {
        return Err(InstallerError::InvalidInput(
            "Username must start with a lowercase letter and contain only lowercase letters, digits, '-' and '_'.".into(),
        ));
    }
    let valid_host = hostname
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let starts_host_ok = hostname
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false);
    if !valid_host || !starts_host_ok || hostname.len() > 63 {
        return Err(InstallerError::InvalidInput(
            "Hostname must contain only lowercase letters, digits, and '-'.".into(),
        ));
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn start_installation(
    app: tauri::AppHandle,
    iso_path: String,
    host_drive_letter: String,
    root_disk_size_gb: u32,
    user_name: String,
    password: String,
    hostname: String,
    locale: String,
    timezone: String,
) -> Result<(), InstallerError> {
    emit(&app, "verify", 0, "Verifying environment");

    if !disk_ops::check_admin_privileges()? {
        return Err(InstallerError::PermissionDenied(
            "Run the installer as Administrator.".into(),
        ));
    }
    if boot_ops::detect_boot_mode()? == boot_ops::BootMode::LegacyBIOS {
        return Err(InstallerError::BootloaderConfig(
            "Legacy BIOS is not supported. UEFI firmware required.".into(),
        ));
    }
    validate_user_input(&user_name, &password, &hostname)?;

    if root_disk_size_gb < image_ops::MIN_ROOT_DISK_GB
        || root_disk_size_gb > image_ops::MAX_ROOT_DISK_GB
    {
        return Err(InstallerError::InvalidInput(format!(
            "root.disk size must be between {} and {} GB.",
            image_ops::MIN_ROOT_DISK_GB,
            image_ops::MAX_ROOT_DISK_GB
        )));
    }

    emit(&app, "iso", 8, "Mounting source ISO");
    let iso_drive = iso_ops::mount_iso(&iso_path)?;
    let kernel_info = match iso_ops::find_linux_kernel(&iso_drive) {
        Ok(k) => k,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };
    let squashfs_rel = match iso_ops::find_squashfs_path(&iso_drive) {
        Ok(s) => s,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };

    let squashfs_bytes = image_ops::read_squashfs_size(&iso_drive, &squashfs_rel)?;
    let root_disk_bytes: u64 = (root_disk_size_gb as u64) * BYTES_PER_GB;

    emit(
        &app,
        "verify",
        12,
        &format!(
            "Validating capacity on {}:\\ for {} GB root.disk + {} MB squashfs",
            host_drive_letter,
            root_disk_size_gb,
            squashfs_bytes / (1024 * 1024)
        ),
    );

    let layout = image_ops::build_layout(&host_drive_letter);
    if let Err(e) = image_ops::validate_capacity(&host_drive_letter, root_disk_bytes, squashfs_bytes)
    {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }
    if let Err(e) = image_ops::ensure_host_dir(&layout) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    emit(
        &app,
        "image",
        22,
        "Allocating raw root.disk (formatted in-flight on first boot)",
    );
    if let Err(e) = image_ops::create_preallocated_root_disk(&layout.root_disk, root_disk_bytes) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    emit(&app, "image", 50, "Copying root payload (squashfs) to host");
    if let Err(e) = image_ops::copy_squashfs_from_iso(&iso_drive, &squashfs_rel, &layout.squashfs) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        return Err(e);
    }

    emit(&app, "image", 62, "Writing provisioning configuration");
    let host_serial = match image_ops::get_ntfs_volume_serial(&host_drive_letter) {
        Ok(s) => s,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            image_ops::remove_host_artifacts(&layout);
            return Err(e);
        }
    };
    if let Err(e) = image_ops::write_provisioning_config(
        &layout,
        &user_name,
        &password,
        &hostname,
        &locale,
        &timezone,
        &host_serial,
    ) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        return Err(e);
    }

    emit(&app, "boot", 68, "Staging kernel and initrd");
    let temp_build = std::env::temp_dir().join(format!(
        "nextos-build-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    if let Err(e) = std::fs::create_dir_all(&temp_build) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        return Err(InstallerError::Io(format!("temp build dir: {}", e)));
    }
    let stock_kernel = temp_build.join("vmlinuz");
    let stock_initrd = temp_build.join("initrd.stock");
    if let Err(e) = iso_ops::copy_iso_file(&iso_drive, &kernel_info.kernel_path, &stock_kernel) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        let _ = std::fs::remove_dir_all(&temp_build);
        return Err(e);
    }
    if let Err(e) = iso_ops::copy_iso_file(&iso_drive, &kernel_info.initrd_path, &stock_initrd) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        let _ = std::fs::remove_dir_all(&temp_build);
        return Err(e);
    }

    emit(&app, "boot", 76, "Building NextOS overlay initrd (cpio)");
    let overlay_gz = temp_build.join("overlay.cpio.gz");
    if let Err(e) = initramfs_ops::build_overlay_cpio_gz(&overlay_gz) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        let _ = std::fs::remove_dir_all(&temp_build);
        return Err(e);
    }

    emit(&app, "boot", 82, "Mounting EFI System Partition");
    let esp_letter = match boot_ops::mount_esp() {
        Ok(l) => l,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            image_ops::remove_host_artifacts(&layout);
            let _ = std::fs::remove_dir_all(&temp_build);
            return Err(e);
        }
    };

    if boot_ops::detect_secure_boot() {
        emit(
            &app,
            "boot",
            83,
            "Secure Boot detected — copying full EFI shim chain for compatibility",
        );
    }

    emit(&app, "boot", 86, "Placing EFI payload on ESP");
    let efi_boot_path = match boot_ops::copy_efi_payload_from_iso(&iso_drive, &esp_letter) {
        Ok(p) => p,
        Err(e) => {
            let _ = boot_ops::cleanup_esp_payload(&esp_letter);
            let _ = iso_ops::unmount_iso(&iso_path);
            image_ops::remove_host_artifacts(&layout);
            let _ = std::fs::remove_dir_all(&temp_build);
            return Err(e);
        }
    };

    let _ = iso_ops::unmount_iso(&iso_path);

    emit(&app, "boot", 90, "Copying kernel, initrd and overlay to host volume");
    if let Err(e) =
        image_ops::place_boot_payload_on_host(&layout, &stock_kernel, &stock_initrd, &overlay_gz)
    {
        let _ = boot_ops::cleanup_esp_payload(&esp_letter);
        image_ops::remove_host_artifacts(&layout);
        let _ = std::fs::remove_dir_all(&temp_build);
        return Err(e);
    }

    let grub_cfg = boot_ops::generate_loop_grub_cfg(&host_serial);
    if let Err(e) = boot_ops::write_grub_cfg(&esp_letter, &grub_cfg) {
        let _ = boot_ops::cleanup_esp_payload(&esp_letter);
        image_ops::remove_host_artifacts(&layout);
        let _ = std::fs::remove_dir_all(&temp_build);
        return Err(e);
    }

    emit(&app, "boot", 94, "Registering UEFI boot entry");
    let nextos_guid = match boot_ops::register_firmware_entry(&esp_letter, &efi_boot_path) {
        Ok(g) => g,
        Err(e) => {
            let _ = boot_ops::cleanup_esp_payload(&esp_letter);
            image_ops::remove_host_artifacts(&layout);
            let _ = std::fs::remove_dir_all(&temp_build);
            return Err(e);
        }
    };

    let _ = std::fs::remove_dir_all(&temp_build);

    // Store the BCD GUID so reboot_now can set the one-time BootNext override
    // Use a small temp file; it's cleaned up after reboot anyway.
    let guid_file = std::env::temp_dir().join("nextos_boot_guid.txt");
    let _ = std::fs::write(&guid_file, &nextos_guid);

    emit(&app, "done", 100, "Installation complete. Reboot to launch NextOS.");
    Ok(())
}

#[tauri::command]
async fn uninstall_nextos(host_drive_letter: String) -> Result<(), InstallerError> {
    if !disk_ops::check_admin_privileges()? {
        return Err(InstallerError::PermissionDenied(
            "Run as Administrator to uninstall.".into(),
        ));
    }
    let _ = boot_ops::cleanup_nextos_firmware_entries();
    if let Ok(esp_letter) = boot_ops::mount_esp() {
        let _ = boot_ops::cleanup_esp_payload(&esp_letter);
    }
    let layout = image_ops::build_layout(&host_drive_letter);
    image_ops::remove_host_artifacts(&layout);
    Ok(())
}

#[tauri::command]
async fn reboot_now() -> Result<(), InstallerError> {
    let guid_path = std::env::temp_dir().join("nextos_boot_guid.txt");
    let guid = std::fs::read_to_string(&guid_path).unwrap_or_default();
    boot_ops::reboot_system(guid.trim())
}

fn emit(app: &tauri::AppHandle, step: &str, progress: u32, message: &str) {
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
            list_host_partitions,
            get_iso_size_mb,
            cleanup_old_boot_entries,
            start_installation,
            uninstall_nextos,
            reboot_now,
        ])
        .run(tauri::generate_context!())
        .expect("Failed to launch application");
}
