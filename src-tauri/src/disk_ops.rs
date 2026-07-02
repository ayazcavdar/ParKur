use crate::error::InstallerError;
use crate::util::run_powershell;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HostCandidate {
    pub disk_number: u32,
    pub partition_number: u32,
    pub drive_letter: String,
    pub label: String,
    pub size_gb: f64,
    pub free_gb: f64,
}

pub fn check_admin_privileges() -> Result<bool, InstallerError> {
    let output = run_powershell(
        "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
    )?;
    Ok(output.trim().eq_ignore_ascii_case("true"))
}

pub fn list_host_candidates() -> Result<Vec<HostCandidate>, InstallerError> {
    let script = r#"
        $ProgressPreference = 'SilentlyContinue'
        $WarningPreference = 'SilentlyContinue'
        $InformationPreference = 'SilentlyContinue'
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
                        label = if ($v.FileSystemLabel) { $v.FileSystemLabel } else { "Local Disk" }
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
    let candidates: Vec<HostCandidate> = serde_json::from_str(trimmed).map_err(|e| {
        InstallerError::JsonParse(format!(
            "candidate parse failed: {} (output: {})",
            e, trimmed
        ))
    })?;
    Ok(candidates)
}
