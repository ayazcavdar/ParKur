
use crate::error::InstallerError;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiskInfo {
    pub disk_number: u32,
    pub total_size_gb: f64,
    pub partitions: Vec<PartitionInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PartitionInfo {
    pub partition_number: u32,
    pub drive_letter: String,
    pub size_gb: f64,
    pub free_space_gb: f64,
    pub file_system: String,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NewPartitionResult {
    pub drive_letter: String,
    pub size_mb: u64,
    pub label: String,
}

pub fn run_powershell(script: &str) -> Result<String, InstallerError> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy", "Bypass",
            "-Command",
            &format!(
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
                script
            ),
        ])
        .output()
        .map_err(|e| {
            InstallerError::CommandExecution(format!(
                "PowerShell baÅŸlatÄ±lamadÄ±: {}. Windows PowerShell yÃ¼klÃ¼ mÃ¼?",
                e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InstallerError::CommandExecution(format!(
            "PowerShell hatasÄ± (Ã§Ä±kÄ±ÅŸ kodu: {:?}): {}",
            output.status.code(),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn run_diskpart(commands: &[&str]) -> Result<String, InstallerError> {
    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join("nextos_diskpart_script.txt");

    let script_content = commands.join("\n");
    std::fs::write(&script_path, &script_content).map_err(|e| {
        InstallerError::Io(format!(
            "diskpart script dosyasÄ± yazÄ±lamadÄ± ({}): {}",
            script_path.display(),
            e
        ))
    })?;

    let output = Command::new("diskpart")
        .args(["/s", &script_path.to_string_lossy()])
        .output();

    let _ = std::fs::remove_file(&script_path);

    let output = output.map_err(|e| {
        InstallerError::CommandExecution(format!(
            "diskpart baÅŸlatÄ±lamadÄ±: {}. YÃ¶netici yetkisiyle Ã§alÄ±ÅŸtÄ±rÄ±yor musunuz?",
            e
        ))
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(InstallerError::DiskOperation(format!(
            "diskpart hatasÄ± (Ã§Ä±kÄ±ÅŸ kodu: {:?}):\nÃ‡Ä±ktÄ±: {}\nHata: {}",
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        )));
    }

    let stdout_lower = stdout.to_lowercase();
    if stdout_lower.contains("hata") || stdout_lower.contains("error") || stdout_lower.contains("baÅŸarÄ±sÄ±z") {
        if !stdout_lower.contains("baÅŸarÄ±yla") && !stdout_lower.contains("successfully") {
            return Err(InstallerError::DiskOperation(format!(
                "diskpart iÅŸlem hatasÄ±:\n{}",
                stdout.trim()
            )));
        }
    }

    Ok(stdout)
}

pub fn check_admin_privileges() -> Result<bool, InstallerError> {
    let output = run_powershell(
        "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"
    )?;

    Ok(output.trim().eq_ignore_ascii_case("true"))
}

pub fn get_disk_info() -> Result<Vec<DiskInfo>, InstallerError> {
    let ps_script = r#"
        $disks = @()
        Get-Disk | Where-Object { $_.OperationalStatus -eq 'Online' } | ForEach-Object {
            $disk = $_
            $parts = @()
            Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue | ForEach-Object {
                $p = $_
                $v = Get-Volume -Partition $p -ErrorAction SilentlyContinue
                $dl = if ($p.DriveLetter) { "$($p.DriveLetter)" } else { "" }
                $fs = if ($v -and $v.FileSystemType) { "$($v.FileSystemType)" } else { "Bilinmiyor" }
                $fl = if ($v -and $v.FileSystemLabel) { "$($v.FileSystemLabel)" } else { "" }
                $fr = if ($v) { [math]::Round($v.SizeRemaining / 1GB, 2) } else { 0 }
                $parts += [PSCustomObject]@{
                    partition_number = [int]$p.PartitionNumber
                    drive_letter     = $dl
                    size_gb          = [math]::Round($p.Size / 1GB, 2)
                    free_space_gb    = $fr
                    file_system      = $fs
                    label            = $fl
                }
            }
            $disks += [PSCustomObject]@{
                disk_number   = [int]$disk.Number
                total_size_gb = [math]::Round($disk.Size / 1GB, 2)
                partitions    = $parts
            }
        }
        if ($disks.Count -eq 0) {
            "[]"
        } elseif ($disks.Count -eq 1) {
            "[" + ($disks | ConvertTo-Json -Depth 3 -Compress) + "]"
        } else {
            $disks | ConvertTo-Json -Depth 3 -Compress
        }
    "#;

    let output = run_powershell(ps_script)?;
    let trimmed = output.trim();

    if trimmed.is_empty() || trimmed == "null" || trimmed == "[]" {
        return Ok(vec![]);
    }

    let values: Vec<serde_json::Value> = serde_json::from_str(trimmed).map_err(|e| {
        InstallerError::JsonParse(format!(
            "Disk bilgisi JSON ayrÄ±ÅŸtÄ±rÄ±lamadÄ±: {}. Ham Ã§Ä±ktÄ±: {}",
            e,
            &trimmed[..trimmed.len().min(200)]
        ))
    })?;

    let mut disks = Vec::new();
    for val in &values {
        let disk_number = val["disk_number"].as_u64().unwrap_or(0) as u32;
        let total_size_gb = val["total_size_gb"].as_f64().unwrap_or(0.0);

        let partitions = parse_partitions(&val["partitions"]);

        disks.push(DiskInfo {
            disk_number,
            total_size_gb,
            partitions,
        });
    }

    Ok(disks)
}

fn parse_partitions(val: &serde_json::Value) -> Vec<PartitionInfo> {
    let items: Vec<&serde_json::Value> = if val.is_array() {
        val.as_array().unwrap().iter().collect()
    } else if val.is_object() {
        vec![val]
    } else {
        return vec![];
    };

    items
        .iter()
        .map(|p| PartitionInfo {
            partition_number: p["partition_number"].as_u64().unwrap_or(0) as u32,
            drive_letter: p["drive_letter"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            size_gb: p["size_gb"].as_f64().unwrap_or(0.0),
            free_space_gb: p["free_space_gb"].as_f64().unwrap_or(0.0),
            file_system: p["file_system"]
                .as_str()
                .unwrap_or("Bilinmiyor")
                .to_string(),
            label: p["label"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

pub fn get_max_shrink_size_mb(drive_letter: &str) -> Result<u64, InstallerError> {
    if drive_letter.is_empty() || drive_letter.len() > 1 {
        return Err(InstallerError::InvalidInput(format!(
            "GeÃ§ersiz sÃ¼rÃ¼cÃ¼ harfi: '{}'. Tek karakter olmalÄ± (Ã¶rn: 'C').",
            drive_letter
        )));
    }

    let script = format!(
        r#"
        $sizes = Get-PartitionSupportedSize -DriveLetter '{}'
        $maxShrinkBytes = $sizes.SizeMax - $sizes.SizeMin
        [math]::Floor($maxShrinkBytes / 1MB)
        "#,
        drive_letter
    );

    let output = run_powershell(&script)?;
    let max_mb: u64 = output.trim().parse().map_err(|e| {
        InstallerError::DiskOperation(format!(
            "KÃ¼Ã§Ã¼ltme boyutu ayrÄ±ÅŸtÄ±rÄ±lamadÄ±: {}. Ã‡Ä±ktÄ±: '{}'",
            e,
            output.trim()
        ))
    })?;

    Ok(max_mb)
}

pub fn find_available_drive_letter() -> Result<String, InstallerError> {
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
            "KullanÄ±labilir sÃ¼rÃ¼cÃ¼ harfi bulunamadÄ± (N-Z arasÄ± tÃ¼mÃ¼ dolu).".into(),
        ));
    }

    Ok(letter)
}
pub fn shrink_and_create_partition(
    disk_number: u32,
    partition_number: u32,
    shrink_size_mb: u64,
) -> Result<NewPartitionResult, InstallerError> {
    if shrink_size_mb < 4096 {
        return Err(InstallerError::InvalidInput(
            "KÃ¼Ã§Ã¼ltme boyutu en az 4096 MB (4 GB) olmalÄ±dÄ±r.".into(),
        ));
    }

    if shrink_size_mb > 500_000 {
        return Err(InstallerError::InvalidInput(
            "KÃ¼Ã§Ã¼ltme boyutu 500 GB'Ä± aÅŸamaz. LÃ¼tfen makul bir deÄŸer girin.".into(),
        ));
    }

    let drive_letter = find_available_drive_letter()?;
    println!(
        "[DISK] SÃ¼rÃ¼cÃ¼ harfi '{}' atanacak. Disk {}, BÃ¶lÃ¼m {}, KÃ¼Ã§Ã¼ltme: {} MB",
        drive_letter, disk_number, partition_number, shrink_size_mb
    );

    let cmd_select_disk = format!("select disk {}", disk_number);
    let cmd_select_part = format!("select partition {}", partition_number);
    let cmd_shrink = format!("shrink desired={}", shrink_size_mb);
    let cmd_assign = format!("assign letter={}", drive_letter);

    let diskpart_commands: Vec<&str> = vec![
        &cmd_select_disk,
        &cmd_select_part,
        &cmd_shrink,
        "create partition primary",
        "format fs=ntfs quick label=\"NextOS_Install\"",
        &cmd_assign,
    ];

    println!("[DISK] diskpart komutlarÄ±: {:?}", diskpart_commands);

    let output = run_diskpart(&diskpart_commands)?;
    println!("[DISK] diskpart Ã§Ä±ktÄ±sÄ±:\n{}", output);

    let output_lower = output.to_lowercase();
    if !output_lower.contains("baÅŸarÄ±yla") && !output_lower.contains("successfully") && !output_lower.contains("percent") {
        println!("[DISK] UYARI: diskpart Ã§Ä±ktÄ±sÄ±nda aÃ§Ä±k baÅŸarÄ± mesajÄ± bulunamadÄ±.");
    }

    Ok(NewPartitionResult {
        drive_letter: drive_letter.clone(),
        size_mb: shrink_size_mb,
        label: "NextOS_Install".into(),
    })
}
