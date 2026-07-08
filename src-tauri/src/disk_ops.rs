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
    pub bitlocker: bool,
    pub has_nextos: bool,
}

/// Single-shot environment probe: gathers everything `start_installation`
/// needs to know about the machine in ONE PowerShell process instead of five
/// (admin, firmware mode, Secure Boot, free space, volume serial).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EnvProbe {
    pub is_admin: bool,
    pub boot_mode: String,
    pub secure_boot: bool,
    pub free_bytes: u64,
    pub volume_serial: String,
    pub bitlocker: bool,
}

pub fn probe_environment(drive_letter: &str) -> Result<EnvProbe, InstallerError> {
    let letter = drive_letter.trim_end_matches(':');
    let script = format!(
        r#"
        $adm = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        $fw = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control' -Name 'PEFirmwareType' -ErrorAction SilentlyContinue).PEFirmwareType
        $bootMode = if ($fw -eq 1) {{ 'BIOS' }} else {{ 'UEFI' }}
        $sb = $false
        try {{ $sb = ((Get-SecureBootUEFI -Name 'SecureBoot' -ErrorAction Stop).Bytes[0] -eq 1) }} catch {{}}
        $free = [uint64](Get-Volume -DriveLetter '{letter}' -ErrorAction Stop).SizeRemaining
        $serial = (Get-CimInstance -ClassName Win32_LogicalDisk -Filter "DeviceID='{letter}:'" -ErrorAction Stop).VolumeSerialNumber
        $bl = $false
        try {{
            $vol = Get-CimInstance -Namespace 'root\cimv2\Security\MicrosoftVolumeEncryption' -ClassName Win32_EncryptableVolume -Filter "DriveLetter='{letter}:'" -ErrorAction Stop
            if ($vol -and $vol.ProtectionStatus -eq 1) {{ $bl = $true }}
        }} catch {{
            try {{ $bl = ((Get-BitLockerVolume -MountPoint '{letter}:' -ErrorAction Stop).ProtectionStatus -eq 'On') }} catch {{}}
        }}
        [PSCustomObject]@{{
            is_admin = [bool]$adm
            boot_mode = $bootMode
            secure_boot = [bool]$sb
            free_bytes = $free
            volume_serial = "$serial"
            bitlocker = [bool]$bl
        }} | ConvertTo-Json -Compress
        "#,
        letter = letter
    );
    let output = run_powershell(&script)?;
    let trimmed = output.trim();
    let mut probe: EnvProbe = serde_json::from_str(trimmed).map_err(|e| {
        InstallerError::JsonParse(format!("env probe parse failed: {} (output: {})", e, trimmed))
    })?;
    probe.volume_serial = probe
        .volume_serial
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_uppercase();
    if probe.volume_serial.is_empty() {
        return Err(InstallerError::DiskOperation(format!(
            "NTFS volume serial query failed for {}:",
            letter
        )));
    }
    Ok(probe)
}

/// True when Windows Fast Startup (hiberboot) is enabled. Fast Startup leaves
/// NTFS volumes in a "dirty" hibernated state that Linux refuses to mount
/// read-write, so the user should disable it before installing.
pub fn check_fast_startup_enabled() -> Result<bool, InstallerError> {
    let output = run_powershell(
        r#"
        try {
            $v = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Power' -Name 'HiberbootEnabled' -ErrorAction Stop).HiberbootEnabled
            if ($v -eq 1) { 'ON' } else { 'OFF' }
        } catch { 'OFF' }
        "#,
    )?;
    Ok(output.trim() == "ON")
}

pub fn detect_secure_boot() -> Result<bool, InstallerError> {
    let output = run_powershell(
        r#"
        try {
            $sb = (Get-SecureBootUEFI -Name 'SecureBoot' -ErrorAction Stop).Bytes[0]
            if ($sb -eq 1) { 'ON' } else { 'OFF' }
        } catch { 'OFF' }
        "#,
    )?;
    Ok(output.trim() == "ON")
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
                    $letter = "$($p.DriveLetter)"
                    $bl = $false
                    try {
                        $ev = Get-CimInstance -Namespace 'root\cimv2\Security\MicrosoftVolumeEncryption' -ClassName Win32_EncryptableVolume -Filter "DriveLetter='$letter`:'" -ErrorAction Stop
                        if ($ev -and $ev.ProtectionStatus -eq 1) { $bl = $true }
                    } catch {
                        try { $bl = ((Get-BitLockerVolume -MountPoint "$letter`:" -ErrorAction Stop).ProtectionStatus -eq 'On') } catch {}
                    }
                    $hasNext = Test-Path "$letter`:\NextOS\root.disk"
                    $results += [PSCustomObject]@{
                        disk_number = [int]$disk.Number
                        partition_number = [int]$p.PartitionNumber
                        drive_letter = $letter
                        label = if ($v.FileSystemLabel) { $v.FileSystemLabel } else { "Local Disk" }
                        size_gb = [math]::Round($v.Size / 1GB, 1)
                        free_gb = [math]::Round($v.SizeRemaining / 1GB, 1)
                        bitlocker = [bool]$bl
                        has_nextos = [bool]$hasNext
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
