use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NewPartitionResult {
    pub drive_letter: String,
    pub size_mb: u64,
}

/// PowerShell komutunu pencere açmadan çalıştırır.
pub fn run_powershell(script: &str) -> Result<String, InstallerError> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!(
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
                script
            ),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| {
            InstallerError::CommandExecution(format!("PowerShell başlatılamadı: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::CommandExecution(format!(
            "PowerShell hatası (çıkış kodu {:?}): {}",
            output.status.code(),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// diskpart komutunu pencere açmadan çalıştırır.
fn run_diskpart(commands: &[&str]) -> Result<String, InstallerError> {
    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join("nextos_diskpart.txt");

    std::fs::write(&script_path, commands.join("\n")).map_err(|e| {
        InstallerError::Io(format!("diskpart script yazılamadı: {}", e))
    })?;

    let output = Command::new("diskpart")
        .args(["/s", &script_path.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let _ = std::fs::remove_file(&script_path);

    let output = output.map_err(|e| {
        InstallerError::CommandExecution(format!("diskpart başlatılamadı: {}", e))
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::DiskOperation(format!(
            "diskpart hatası:\n{}\n{}",
            stdout.trim(),
            stderr.trim()
        )));
    }

    Ok(stdout)
}

/// Yönetici yetkisini kontrol eder.
pub fn check_admin_privileges() -> Result<bool, InstallerError> {
    let output = run_powershell(
        "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
    )?;
    Ok(output.trim().eq_ignore_ascii_case("true"))
}

/// En uygun NTFS bölümünü otomatik bulur.
/// `min_free_mb` kadar boş alanı olan, en büyük boş alana sahip bölümü seçer.
pub fn find_best_partition(min_free_mb: u64) -> Result<(u32, u32, String), InstallerError> {
    let script = format!(
        r#"
        $minFree = {min_free}
        $best = $null
        $bestFree = 0
        Get-Disk | Where-Object {{ $_.OperationalStatus -eq 'Online' }} | ForEach-Object {{
            $disk = $_
            Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue | ForEach-Object {{
                $p = $_
                $v = Get-Volume -Partition $p -ErrorAction SilentlyContinue
                if ($v -and $p.DriveLetter -and $v.FileSystemType -eq 'NTFS') {{
                    $freeMB = [math]::Floor($v.SizeRemaining / 1MB)
                    if ($freeMB -gt $bestFree -and $freeMB -ge $minFree) {{
                        $best = [PSCustomObject]@{{
                            disk = [int]$disk.Number
                            part = [int]$p.PartitionNumber
                            letter = "$($p.DriveLetter)"
                            free = $freeMB
                        }}
                        $bestFree = $freeMB
                    }}
                }}
            }}
        }}
        if ($best) {{ "$($best.disk)|$($best.part)|$($best.letter)|$($best.free)" }}
        else {{ "NONE" }}
        "#,
        min_free = min_free_mb
    );

    let output = run_powershell(&script)?;
    let trimmed = output.trim();

    if trimmed == "NONE" {
        return Err(InstallerError::DiskOperation(format!(
            "Uygun bölüm bulunamadı. En az {} MB boş alana sahip bir NTFS bölümü gerekli.",
            min_free_mb
        )));
    }

    let parts: Vec<&str> = trimmed.split('|').collect();
    if parts.len() != 4 {
        return Err(InstallerError::DiskOperation(format!(
            "Bölüm bilgisi ayrıştırılamadı: '{}'",
            trimmed
        )));
    }

    let disk_num: u32 = parts[0].parse().unwrap_or(0);
    let part_num: u32 = parts[1].parse().unwrap_or(0);
    let letter = parts[2].to_string();

    println!(
        "[DISK] En uygun bölüm: Disk {} Bölüm {} ({}:\\, {} MB boş)",
        disk_num, part_num, letter, parts[3]
    );

    Ok((disk_num, part_num, letter))
}

/// Kullanılabilir sürücü harfi bulur (N-Z arası).
fn find_available_drive_letter() -> Result<String, InstallerError> {
    let script = r#"
        $used = @()
        Get-Volume | ForEach-Object { if ($_.DriveLetter) { $used += $_.DriveLetter } }
        Get-Partition | ForEach-Object { if ($_.DriveLetter) { $used += $_.DriveLetter } }
        $available = 78..90 | ForEach-Object { [char]$_ } | Where-Object { $_ -notin $used }
        if ($available.Count -gt 0) { $available[0] } else { "" }
    "#;

    let output = run_powershell(script)?;
    let letter = output.trim().to_string();

    if letter.is_empty() || letter.len() != 1 {
        return Err(InstallerError::DiskOperation(
            "Kullanılabilir sürücü harfi bulunamadı.".into(),
        ));
    }

    Ok(letter)
}

/// Bölümü küçültür ve yeni NTFS bölüm oluşturur.
pub fn shrink_and_create_partition(
    disk_number: u32,
    partition_number: u32,
    shrink_mb: u64,
) -> Result<NewPartitionResult, InstallerError> {
    let drive_letter = find_available_drive_letter()?;

    println!(
        "[DISK] Küçültme: Disk {} Bölüm {} -> {} MB, harf: {}",
        disk_number, partition_number, shrink_mb, drive_letter
    );

    let cmd_disk = format!("select disk {}", disk_number);
    let cmd_part = format!("select partition {}", partition_number);
    let cmd_shrink = format!("shrink desired={}", shrink_mb);
    let cmd_assign = format!("assign letter={}", drive_letter);

    let commands: Vec<&str> = vec![
        &cmd_disk,
        &cmd_part,
        &cmd_shrink,
        "create partition primary",
        "format fs=ntfs quick label=\"NextOS_Install\"",
        &cmd_assign,
    ];

    let output = run_diskpart(&commands)?;
    println!("[DISK] diskpart çıktısı:\n{}", output);

    Ok(NewPartitionResult {
        drive_letter,
        size_mb: shrink_mb,
    })
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PartitionInfo {
    pub disk_number: u32,
    pub partition_number: u32,
    pub drive_letter: String,
    pub label: String,
    pub size_gb: f64,
    pub free_gb: f64,
}

/// Tüm NTFS bölümlerini listeler.
pub fn list_partitions() -> Result<Vec<PartitionInfo>, InstallerError> {
    let script = r#"
        $results = @()
        Get-Disk | Where-Object { $_.OperationalStatus -eq 'Online' } | ForEach-Object {
            $disk = $_
            Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue | ForEach-Object {
                $p = $_
                $v = Get-Volume -Partition $p -ErrorAction SilentlyContinue
                if ($v -and $p.DriveLetter -and $v.FileSystemType -eq 'NTFS') {
                    $results += [PSCustomObject]@{
                        disk_number = [int]$disk.Number
                        partition_number = [int]$p.PartitionNumber
                        drive_letter = "$($p.DriveLetter)"
                        label = if ($v.FileSystemLabel) { $v.FileSystemLabel } else { "Yerel Disk" }
                        size_gb = [math]::Round($v.Size / 1GB, 1)
                        free_gb = [math]::Round($v.SizeRemaining / 1GB, 1)
                    }
                }
            }
        }
        if ($results.Count -eq 0) { "[]" }
        elseif ($results.Count -eq 1) { "[$($results | ConvertTo-Json -Compress)]" }
        else { $results | ConvertTo-Json -Compress }
    "#;

    let output = run_powershell(script)?;
    let trimmed = output.trim();

    if trimmed.is_empty() || trimmed == "[]" || trimmed == "null" {
        return Ok(vec![]);
    }

    let partitions: Vec<PartitionInfo> = serde_json::from_str(trimmed).map_err(|e| {
        InstallerError::JsonParse(format!(
            "Bölüm bilgisi ayrıştırılamadı: {} (çıktı: {})",
            e, trimmed
        ))
    })?;

    Ok(partitions)
}
