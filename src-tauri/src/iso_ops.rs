
use crate::disk_ops::run_powershell;
use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use std::process::Command;

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
    ("install.amd/vmlinuz", "install.amd/initrd.gz"),
    ("d-i/vmlinuz", "d-i/initrd.gz"),
];

pub fn mount_iso(iso_path: &str) -> Result<String, InstallerError> {
    if !std::path::Path::new(iso_path).exists() {
        return Err(InstallerError::InvalidInput(format!(
            "ISO dosyasÄ± bulunamadÄ±: {}",
            iso_path
        )));
    }

    if !iso_path.to_lowercase().ends_with(".iso") {
        return Err(InstallerError::InvalidInput(
            "SeÃ§ilen dosya bir ISO dosyasÄ± deÄŸil (.iso uzantÄ±lÄ± olmalÄ±).".into(),
        ));
    }

    let check_script = format!(
        r#"
        $existing = Get-DiskImage -ImagePath '{}' -ErrorAction SilentlyContinue
        if ($existing -and $existing.Attached) {{
            $vol = $existing | Get-Volume -ErrorAction SilentlyContinue
            if ($vol -and $vol.DriveLetter) {{
                $vol.DriveLetter
            }} else {{
                "MOUNTED_NO_LETTER"
            }}
        }} else {{
            "NOT_MOUNTED"
        }}
        "#,
        iso_path.replace('\'', "''")
    );

    let check_result = run_powershell(&check_script)?;
    let check_trimmed = check_result.trim();

    if check_trimmed != "NOT_MOUNTED" && check_trimmed != "MOUNTED_NO_LETTER" {
        println!("[ISO] ISO zaten monte edilmiÅŸ: sÃ¼rÃ¼cÃ¼ {}", check_trimmed);
        return Ok(check_trimmed.to_string());
    }

    if check_trimmed == "MOUNTED_NO_LETTER" {
        let _ = unmount_iso(iso_path);
    }

    let mount_script = format!(
        r#"
        $img = Mount-DiskImage -ImagePath '{}' -PassThru -ErrorAction Stop
        Start-Sleep -Milliseconds 1500
        $vol = $img | Get-Volume -ErrorAction Stop
        if ($vol.DriveLetter) {{
            $vol.DriveLetter
        }} else {{
            throw "ISO monte edildi ancak sÃ¼rÃ¼cÃ¼ harfi atanamadÄ±."
        }}
        "#,
        iso_path.replace('\'', "''")
    );

    let output = run_powershell(&mount_script).map_err(|e| {
        InstallerError::IsoExtraction(format!(
            "ISO monte edilemedi ({}): {}",
            iso_path, e
        ))
    })?;

    let drive_letter = output.trim().to_string();

    if drive_letter.len() != 1 || !drive_letter.chars().next().unwrap_or(' ').is_ascii_alphabetic()
    {
        return Err(InstallerError::IsoExtraction(format!(
            "ISO monte edildi ancak geÃ§erli sÃ¼rÃ¼cÃ¼ harfi alÄ±namadÄ±. Ã‡Ä±ktÄ±: '{}'",
            drive_letter
        )));
    }

    println!("[ISO] ISO baÅŸarÄ±yla monte edildi: {} -> {}", iso_path, drive_letter);
    Ok(drive_letter)
}

pub fn unmount_iso(iso_path: &str) -> Result<(), InstallerError> {
    let script = format!(
        "Dismount-DiskImage -ImagePath '{}' -ErrorAction SilentlyContinue",
        iso_path.replace('\'', "''")
    );

    run_powershell(&script).map_err(|e| {
        InstallerError::IsoExtraction(format!("ISO demonte edilemedi: {}", e))
    })?;

    println!("[ISO] ISO demonte edildi: {}", iso_path);
    Ok(())
}

pub fn copy_iso_contents(
    iso_drive_letter: &str,
    target_drive_letter: &str,
) -> Result<(), InstallerError> {
    let source = format!("{}:\\", iso_drive_letter);
    let dest = format!("{}:\\", target_drive_letter);

    println!("[ISO] Dosya kopyalama baÅŸlÄ±yor: {} -> {}", source, dest);

    let output = Command::new("robocopy")
        .args([
            &source,
            &dest,
            "/E",        // Alt dizinler dahil
            "/R:3",      // 3 tekrar denemesi
            "/W:1",      // 1 saniye bekleme
            "/NFL",      // Dosya listesi gizle
            "/NDL",      // Dizin listesi gizle
            "/NJH",      // Ä°ÅŸ baÅŸlÄ±ÄŸÄ± gizle
            "/NJS",      // Ä°ÅŸ Ã¶zeti gizle
            "/MT:4",     // 4 thread ile paralel kopyalama
        ])
        .output()
        .map_err(|e| {
            InstallerError::IsoExtraction(format!(
                "robocopy baÅŸlatÄ±lamadÄ±: {}. Windows robocopy yÃ¼klÃ¼ mÃ¼?",
                e
            ))
        })?;

    let exit_code = output.status.code().unwrap_or(-1);

    if exit_code >= 8 {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::IsoExtraction(format!(
            "robocopy dosya kopyalama hatasÄ± (Ã§Ä±kÄ±ÅŸ kodu: {}):\nÃ‡Ä±ktÄ±: {}\nHata: {}",
            exit_code,
            stdout.chars().take(500).collect::<String>(),
            stderr.chars().take(500).collect::<String>()
        )));
    }

    println!(
        "[ISO] Dosya kopyalama tamamlandÄ± (robocopy Ã§Ä±kÄ±ÅŸ kodu: {})",
        exit_code
    );
    Ok(())
}

pub fn find_linux_kernel(partition_root: &str) -> Result<LinuxKernelInfo, InstallerError> {
    let root = std::path::Path::new(partition_root);

    for (kernel_rel, initrd_rel) in KERNEL_SEARCH_PATHS {
        let kernel_path = root.join(kernel_rel);
        let initrd_path = root.join(initrd_rel);

        if kernel_path.exists() && initrd_path.exists() {
            println!(
                "[ISO] Linux Ã§ekirdeÄŸi bulundu: kernel={}, initrd={}",
                kernel_rel, initrd_rel
            );
            return Ok(LinuxKernelInfo {
                kernel_path: kernel_rel.to_string(),
                initrd_path: initrd_rel.to_string(),
            });
        }

        if kernel_path.exists() {
            println!(
                "[ISO] vmlinuz bulundu ({}), ancak eÅŸleÅŸen initrd ({}) bulunamadÄ±.",
                kernel_rel, initrd_rel
            );
        }
    }

    println!("[ISO] Bilinen yollarda Ã§ekirdek bulunamadÄ±, recursive arama yapÄ±lÄ±yor...");
    let found = find_kernel_recursive(root)?;

    match found {
        Some(info) => Ok(info),
        None => Err(InstallerError::IsoExtraction(format!(
            "Linux Ã§ekirdek dosyalarÄ± (vmlinuz/initrd) ISO iÃ§eriÄŸinde bulunamadÄ±. \
             LÃ¼tfen geÃ§erli bir Debian tabanlÄ± Linux ISO dosyasÄ± seÃ§tiÄŸinizden emin olun. \
             Aranan dizin: {}",
            partition_root
        ))),
    }
}

fn find_kernel_recursive(dir: &std::path::Path) -> Result<Option<LinuxKernelInfo>, InstallerError> {
    let mut vmlinuz_path: Option<String> = None;
    let mut initrd_path: Option<String> = None;

    fn search_dir(
        dir: &std::path::Path,
        root: &std::path::Path,
        vmlinuz: &mut Option<String>,
        initrd: &mut Option<String>,
        depth: u32,
    ) {
        if depth > 5 {
            return; // Ã‡ok derin aramayÄ± engelle
        }

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase();

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
                    search_dir(&path, root, vmlinuz, initrd, depth + 1);
                }

                if vmlinuz.is_some() && initrd.is_some() {
                    return;
                }
            }
        }
    }

    search_dir(dir, dir, &mut vmlinuz_path, &mut initrd_path, 0);

    match (vmlinuz_path, initrd_path) {
        (Some(k), Some(i)) => {
            println!("[ISO] Recursive arama ile bulundu: kernel={}, initrd={}", k, i);
            Ok(Some(LinuxKernelInfo {
                kernel_path: k,
                initrd_path: i,
            }))
        }
        _ => Ok(None),
    }
}
