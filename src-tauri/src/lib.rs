mod boot_ops;
mod disk_ops;
mod error;
mod i18n;
mod image_ops;
mod initramfs_ops;
mod iso_ops;
mod squashfs_ops;
mod util;

use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use sha_crypt::{PasswordHasher, ShaCrypt};
use std::path::Path;
use tauri::Emitter;
#[cfg(debug_assertions)]
use tauri::Manager;

const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

/// Hash the user's password as SHA-512 crypt (`$6$rounds=5000$salt$hash`) so
/// only the hash ever leaves this process — the plaintext is never written to
/// disk. The hash is applied on first boot via `chpasswd -e`.
fn hash_password_sha512(password: &str) -> Result<String, InstallerError> {
    let hash = ShaCrypt::SHA512
        .hash_password(password.as_bytes())
        .map_err(|e| {
            InstallerError::CommandExecution(format!("password hashing failed: {}", e))
        })?;
    Ok(hash.as_str().to_string())
}

#[derive(Clone, Serialize, Deserialize)]
struct ProgressPayload {
    step: String,
    progress: u32,
    message_key: String,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    message_params: std::collections::HashMap<String, String>,
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
async fn check_fast_startup() -> Result<bool, InstallerError> {
    disk_ops::check_fast_startup_enabled()
}

#[tauri::command]
async fn list_host_partitions() -> Result<Vec<disk_ops::HostCandidate>, InstallerError> {
    disk_ops::list_host_candidates()
}

#[tauri::command]
async fn get_iso_size_mb(path: String) -> Result<u64, InstallerError> {
    iso_ops::get_iso_size_mb(&path)
}

#[tauri::command]
async fn probe_iso_layout(path: String) -> Result<iso_ops::IsoLayoutInfo, InstallerError> {
    iso_ops::probe_iso_layout(&path)
}

#[tauri::command]
async fn compute_disk_limits(
    free_bytes: u64,
    squashfs_compressed_mb: u64,
    squashfs_uncompressed_mb: u64,
) -> Result<image_ops::DiskLimits, InstallerError> {
    Ok(image_ops::compute_disk_limits(
        free_bytes,
        squashfs_compressed_mb.saturating_mul(1024 * 1024),
        squashfs_uncompressed_mb.saturating_mul(1024 * 1024),
    ))
}

#[tauri::command]
async fn detect_secure_boot() -> Result<bool, InstallerError> {
    disk_ops::detect_secure_boot()
}

#[tauri::command]
async fn disable_fast_startup() -> Result<(), InstallerError> {
    disk_ops::disable_fast_startup()
}

#[tauri::command]
async fn fix_secure_boot() -> Result<disk_ops::SecureBootFixResult, InstallerError> {
    disk_ops::fix_secure_boot()
}

fn validate_user_input(
    user_name: &str,
    password: &str,
    hostname: &str,
) -> Result<(), InstallerError> {
    if user_name.is_empty() {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_USERNAME_EMPTY",
            &[],
        ));
    }
    if password.len() < 4 {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_PASSWORD_SHORT",
            &[],
        ));
    }
    if hostname.is_empty() {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_HOSTNAME_EMPTY",
            &[],
        ));
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
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_USERNAME_INVALID",
            &[],
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
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_HOSTNAME_INVALID",
            &[],
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
    emit_key(&app, "verify", 0, "progress.verify_env", &[]);

    // One PowerShell process for all environment facts (admin, firmware,
    // Secure Boot, free space, volume serial) instead of five.
    let probe = disk_ops::probe_environment(&host_drive_letter)?;
    if !probe.is_admin {
        return Err(InstallerError::coded(
            InstallerError::PermissionDenied,
            "ERR_ADMIN_REQUIRED",
            &[],
        ));
    }
    if probe.boot_mode == "BIOS" {
        return Err(InstallerError::coded(
            InstallerError::BootloaderConfig,
            "ERR_LEGACY_BIOS",
            &[],
        ));
    }
    if probe.bitlocker {
        let drive = host_drive_letter.trim_end_matches(':');
        return Err(InstallerError::coded(
            InstallerError::DiskOperation,
            "ERR_BITLOCKER",
            &[("drive", drive)],
        ));
    }
    if disk_ops::check_fast_startup_enabled()? {
        return Err(InstallerError::coded(
            InstallerError::DiskOperation,
            "ERR_FAST_STARTUP",
            &[],
        ));
    }
    if image_ops::host_has_nextos_install(&host_drive_letter) {
        let drive = host_drive_letter.trim_end_matches(':');
        return Err(InstallerError::coded(
            InstallerError::DiskOperation,
            "ERR_NEXTOS_EXISTS",
            &[("drive", drive)],
        ));
    }
    validate_user_input(&user_name, &password, &hostname)?;
    let password_hash = hash_password_sha512(&password)?;

    emit_key(&app, "iso", 8, "progress.mount_iso", &[]);
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

    let squashfs_bytes = match image_ops::read_squashfs_size(&iso_drive, &squashfs_rel) {
        Ok(b) => b,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };
    let squashfs_host = format!(
        "{}:\\{}",
        iso_drive.trim_end_matches(':'),
        squashfs_rel.replace('/', "\\")
    );
    let squashfs_uncompressed = match squashfs_ops::read_uncompressed_size(Path::new(&squashfs_host)) {
        Ok(b) => b,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            return Err(e);
        }
    };
    let root_disk_bytes: u64 = (root_disk_size_gb as u64) * BYTES_PER_GB;
    if let Err(e) = image_ops::validate_root_disk_size(root_disk_bytes, squashfs_uncompressed) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    emit_key(
        &app,
        "verify",
        12,
        "progress.validate_capacity",
        &[
            ("drive", host_drive_letter.trim_end_matches(':')),
            ("root_gb", &root_disk_size_gb.to_string()),
            ("squash_mb", &(squashfs_bytes / (1024 * 1024)).to_string()),
            ("extracted_mb", &(squashfs_uncompressed / (1024 * 1024)).to_string()),
        ],
    );

    let layout = image_ops::build_layout(&host_drive_letter);
    if let Err(e) = image_ops::validate_capacity(
        &host_drive_letter,
        probe.free_bytes,
        root_disk_bytes,
        squashfs_bytes,
    ) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }
    if let Err(e) = image_ops::ensure_host_dir(&layout) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }
    if let Err(e) = boot_ops::backup_firmware_boot_timeout_to_host(&layout.host_dir) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    emit_key(
        &app,
        "image",
        22,
        "progress.alloc_root_disk",
        &[],
    );
    if let Err(e) = image_ops::create_preallocated_root_disk(&layout.root_disk, root_disk_bytes) {
        let _ = iso_ops::unmount_iso(&iso_path);
        return Err(e);
    }

    emit_key(&app, "image", 50, "progress.copy_squashfs", &[]);
    // Map copy progress onto the 50–62% band with real MB counters.
    let mut last_pct: u32 = 0;
    let copy_result = image_ops::copy_squashfs_from_iso(
        &iso_drive,
        &squashfs_rel,
        &layout.squashfs,
        |done, total| {
            let pct = 50 + ((done as f64 / total.max(1) as f64) * 12.0) as u32;
            if pct > last_pct {
                last_pct = pct;
                emit_key(
                    &app,
                    "image",
                    pct,
                    "progress.copy_squashfs_pct",
                    &[
                        ("done_mb", &(done / (1024 * 1024)).to_string()),
                        ("total_mb", &(total / (1024 * 1024)).to_string()),
                    ],
                );
            }
        },
    );
    if let Err(e) = copy_result {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        return Err(e);
    }

    emit_key(&app, "image", 62, "progress.write_config", &[]);
    let host_serial = probe.volume_serial.clone();
    if let Err(e) = image_ops::write_provisioning_config(
        &layout,
        &user_name,
        &password_hash,
        &hostname,
        &locale,
        &timezone,
        &host_serial,
    ) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        return Err(e);
    }

    emit_key(&app, "boot", 68, "progress.stage_kernel", &[]);
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

    emit_key(&app, "boot", 76, "progress.build_overlay", &[]);
    let overlay_gz = temp_build.join("overlay.cpio.gz");
    if let Err(e) = initramfs_ops::build_overlay_cpio_gz(&overlay_gz, &password) {
        let _ = iso_ops::unmount_iso(&iso_path);
        image_ops::remove_host_artifacts(&layout);
        let _ = std::fs::remove_dir_all(&temp_build);
        return Err(e);
    }

    emit_key(&app, "boot", 82, "progress.mount_esp", &[]);
    let esp_letter = match boot_ops::mount_esp() {
        Ok(l) => l,
        Err(e) => {
            let _ = iso_ops::unmount_iso(&iso_path);
            image_ops::remove_host_artifacts(&layout);
            let _ = std::fs::remove_dir_all(&temp_build);
            return Err(e);
        }
    };

    if probe.secure_boot {
        emit_key(&app, "boot", 83, "progress.secure_boot_shim", &[]);
    }

    emit_key(&app, "boot", 86, "progress.place_efi", &[]);
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

    emit_key(&app, "boot", 90, "progress.copy_boot_host", &[]);
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

    emit_key(&app, "boot", 94, "progress.register_uefi", &[]);
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

    emit_key(&app, "done", 100, "progress.done", &[]);
    Ok(())
}

#[tauri::command]
async fn uninstall_nextos(host_drive_letter: String) -> Result<(), InstallerError> {
    if !disk_ops::check_admin_privileges()? {
        return Err(InstallerError::coded(
            InstallerError::PermissionDenied,
            "ERR_UNINSTALL_ADMIN",
            &[],
        ));
    }
    let letter = host_drive_letter.trim().trim_end_matches(':');
    if !image_ops::host_has_nextos_install(letter) {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_UNINSTALL_NOT_FOUND",
            &[("drive", letter)],
        ));
    }
    let _ = boot_ops::cleanup_nextos_firmware_entries();
    if let Ok(esp_letter) = boot_ops::mount_esp() {
        let _ = boot_ops::cleanup_esp_payload(&esp_letter);
    }
    let layout = image_ops::build_layout(&host_drive_letter);
    let _ = boot_ops::restore_firmware_boot_timeout_from_host(&layout.host_dir);
    image_ops::remove_host_artifacts(&layout);
    Ok(())
}

#[tauri::command]
async fn reboot_now() -> Result<(), InstallerError> {
    let guid_path = std::env::temp_dir().join("nextos_boot_guid.txt");
    let guid = std::fs::read_to_string(&guid_path).unwrap_or_default();
    boot_ops::reboot_system(guid.trim())
}

fn emit_key(
    app: &tauri::AppHandle,
    step: &str,
    progress: u32,
    message_key: &str,
    message_params: &[(&str, &str)],
) {
    let mut params = std::collections::HashMap::new();
    for (k, v) in message_params {
        params.insert((*k).to_string(), (*v).to_string());
    }
    let _ = app.emit(
        "installation-progress",
        ProgressPayload {
            step: step.to_string(),
            progress,
            message_key: message_key.to_string(),
            message_params: params,
        },
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn password_hash_roundtrip() {
        use sha_crypt::{PasswordVerifier, ShaCrypt};
        let hash = super::hash_password_sha512("Dene me123!$#").unwrap();
        assert!(hash.starts_with("$6$"), "unexpected hash format: {}", hash);
        ShaCrypt::SHA512
            .verify_password(b"Dene me123!$#", hash.as_str())
            .expect("hash must verify against the original password");
    }

    #[test]
    fn validate_rejects_short_password() {
        assert!(super::validate_user_input("ayaz", "abc", "pc").is_err());
        assert!(super::validate_user_input("ayaz", "abcd", "pc").is_ok());
    }
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
            check_fast_startup,
            list_host_partitions,
            get_iso_size_mb,
            probe_iso_layout,
            compute_disk_limits,
            detect_secure_boot,
            disable_fast_startup,
            fix_secure_boot,
            cleanup_old_boot_entries,
            start_installation,
            uninstall_nextos,
            reboot_now,
        ])
        .run(tauri::generate_context!())
        .expect("Failed to launch application");
}
