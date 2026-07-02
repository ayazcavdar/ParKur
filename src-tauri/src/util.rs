use crate::error::InstallerError;
use std::os::windows::process::CommandExt;
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
