use crate::disk_ops::run_powershell;
use crate::error::InstallerError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LinuxKernelInfo {
    pub kernel_path: String,
    pub initrd_path: String,
}

const KERNEL_SEARCH_PATHS: &[(&str, &str)] = &[
    ("live/vmlinuz", "live/initrd.img"),
    ("live/vmlinuz", "live/initrd"),
    ("casper/vmlinuz", "casper/initrd"),
    ("casper/vmlinuz", "casper/initrd.lz"),
    ("casper/vmlinuz", "casper/initrd.gz"),
    ("boot/vmlinuz", "boot/initrd.img"),
];

/// ISO dosyasını Windows'a monte eder, sürücü harfini döndürür.
pub fn mount_iso(iso_path: &str) -> Result<String, InstallerError> {
    if !std::path::Path::new(iso_path).exists() {
        return Err(InstallerError::InvalidInput(format!(
            "ISO dosyası bulunamadı: {}",
            iso_path
        )));
    }

    if !iso_path.to_lowercase().ends_with(".iso") {
        return Err(InstallerError::InvalidInput(
            "Seçilen dosya bir ISO dosyası değil.".into(),
        ));
    }

    // Önceden monte edilmişse kaldır
    let _ = unmount_iso(iso_path);

    let script = format!(
        r#"
        $img = Mount-DiskImage -ImagePath '{}' -PassThru -ErrorAction Stop
        Start-Sleep -Milliseconds 1500
        $vol = $img | Get-Volume -ErrorAction Stop
        if ($vol.DriveLetter) {{ $vol.DriveLetter }}
        else {{ throw "ISO monte edildi ancak sürücü harfi atanamadı." }}
        "#,
        iso_path.replace('\'', "''")
    );

    let output = run_powershell(&script).map_err(|e| {
        InstallerError::IsoExtraction(format!("ISO monte edilemedi: {}", e))
    })?;

    let letter = output.trim().to_string();
    if letter.len() != 1 || !letter.chars().next().unwrap_or(' ').is_ascii_alphabetic() {
        return Err(InstallerError::IsoExtraction(format!(
            "Geçerli sürücü harfi alınamadı: '{}'",
            letter
        )));
    }

    println!("[ISO] Monte edildi: {} -> {}:\\", iso_path, letter);
    Ok(letter)
}

/// ISO'yu demonte eder.
pub fn unmount_iso(iso_path: &str) -> Result<(), InstallerError> {
    let script = format!(
        "Dismount-DiskImage -ImagePath '{}' -ErrorAction SilentlyContinue",
        iso_path.replace('\'', "''")
    );
    let _ = run_powershell(&script);
    Ok(())
}

/// ISO dosyasını hedef bölüme `install.iso` olarak kopyalar.
pub fn copy_iso_to_partition(iso_path: &str, target_letter: &str) -> Result<(), InstallerError> {
    let dest = format!("{}:\\install.iso", target_letter);
    println!("[ISO] Kopyalanıyor: {} -> {}", iso_path, dest);

    std::fs::copy(iso_path, &dest).map_err(|e| {
        InstallerError::IsoExtraction(format!("ISO kopyalanamadı: {}", e))
    })?;

    println!("[ISO] Kopyalama tamamlandı: {}", dest);
    Ok(())
}

/// Monte edilmiş ISO içinde Linux çekirdek dosyalarını (vmlinuz/initrd) arar.
pub fn find_linux_kernel(iso_drive_letter: &str) -> Result<LinuxKernelInfo, InstallerError> {
    let root = format!("{}:\\", iso_drive_letter);
    let root_path = std::path::Path::new(&root);

    for (kernel_rel, initrd_rel) in KERNEL_SEARCH_PATHS {
        let kernel_path = root_path.join(kernel_rel);
        let initrd_path = root_path.join(initrd_rel);

        if kernel_path.exists() && initrd_path.exists() {
            println!("[ISO] Çekirdek bulundu: {}, {}", kernel_rel, initrd_rel);
            return Ok(LinuxKernelInfo {
                kernel_path: kernel_rel.to_string(),
                initrd_path: initrd_rel.to_string(),
            });
        }
    }

    search_kernel_recursive(root_path)
}

fn search_kernel_recursive(root: &std::path::Path) -> Result<LinuxKernelInfo, InstallerError> {
    let mut vmlinuz: Option<String> = None;
    let mut initrd: Option<String> = None;

    scan_dir(root, root, &mut vmlinuz, &mut initrd, 0);

    match (vmlinuz, initrd) {
        (Some(k), Some(i)) => {
            println!("[ISO] Recursive arama ile bulundu: {}, {}", k, i);
            Ok(LinuxKernelInfo {
                kernel_path: k,
                initrd_path: i,
            })
        }
        _ => Err(InstallerError::IsoExtraction(
            "Linux çekirdek dosyaları (vmlinuz/initrd) ISO içinde bulunamadı. \
             Geçerli bir Pardus/Debian tabanlı ISO seçtiğinizden emin olun."
                .into(),
        )),
    }
}

fn scan_dir(
    dir: &std::path::Path,
    root: &std::path::Path,
    vmlinuz: &mut Option<String>,
    initrd: &mut Option<String>,
    depth: u32,
) {
    if depth > 4 || (vmlinuz.is_some() && initrd.is_some()) {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();

        if path.is_file() {
            if name.starts_with("vmlinuz") && vmlinuz.is_none() {
                if let Ok(rel) = path.strip_prefix(root) {
                    *vmlinuz = Some(rel.to_string_lossy().replace('\\', "/"));
                }
            } else if (name.starts_with("initrd") || name.starts_with("initramfs"))
                && initrd.is_none()
            {
                if let Ok(rel) = path.strip_prefix(root) {
                    *initrd = Some(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        } else if path.is_dir() {
            scan_dir(&path, root, vmlinuz, initrd, depth + 1);
        }
    }
}
