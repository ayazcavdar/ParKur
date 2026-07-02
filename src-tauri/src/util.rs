use crate::error::InstallerError;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn to_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

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
        .map_err(|e| InstallerError::CommandExecution(format!("powershell spawn failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(InstallerError::CommandExecution(format!(
            "powershell exit {:?}: {} {}",
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn copy_file_to_protected_path(
    src: &Path,
    dest_path: &str,
) -> Result<(), InstallerError> {
    let parent = Path::new(dest_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let script = format!(
        "New-Item -Path '{}' -ItemType Directory -Force -ErrorAction SilentlyContinue | Out-Null; Copy-Item -Path '{}' -Destination '{}' -Force -ErrorAction Stop",
        parent.replace("'", "''"),
        src.to_string_lossy().replace("'", "''"),
        dest_path.replace("'", "''")
    );
    run_powershell(&script).map(|_| ())
}

pub fn write_lf_file_to_protected_path(
    dest_path: &str,
    content: &str,
) -> Result<(), InstallerError> {
    let lf = to_lf(content);
    let temp = std::env::temp_dir().join(format!(
        "nextos_proto_{}.tmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&temp, lf.as_bytes())
        .map_err(|e| InstallerError::Io(format!("Temp write failed: {}", e)))?;
    let res = copy_file_to_protected_path(&temp, dest_path);
    let _ = std::fs::remove_file(&temp);
    res
}

pub fn extract_first_guid(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let rest = &text[start..];
    let end = rest.find('}')? + start + 1;
    let guid = &text[start..end];
    if guid.len() == 38 && guid.contains('-') {
        Some(guid.to_string())
    } else {
        None
    }
}
