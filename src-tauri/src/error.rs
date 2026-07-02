use serde::Serialize;
use std::fmt;

#[derive(Debug)]
pub enum InstallerError {
    DiskOperation(String),
    IsoExtraction(String),
    BootloaderConfig(String),
    PermissionDenied(String),
    CommandExecution(String),
    InvalidInput(String),
    Io(String),
    JsonParse(String),
    ImageOperation(String),
    InitramfsBuild(String),
}

impl fmt::Display for InstallerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiskOperation(m) => write!(f, "Disk operation error: {}", m),
            Self::IsoExtraction(m) => write!(f, "ISO extraction error: {}", m),
            Self::BootloaderConfig(m) => write!(f, "Bootloader configuration error: {}", m),
            Self::PermissionDenied(m) => write!(f, "Permission denied: {}", m),
            Self::CommandExecution(m) => write!(f, "Command execution error: {}", m),
            Self::InvalidInput(m) => write!(f, "Invalid input: {}", m),
            Self::Io(m) => write!(f, "I/O error: {}", m),
            Self::JsonParse(m) => write!(f, "JSON parse error: {}", m),
            Self::ImageOperation(m) => write!(f, "Image operation error: {}", m),
            Self::InitramfsBuild(m) => write!(f, "Initramfs build error: {}", m),
        }
    }
}

impl std::error::Error for InstallerError {}

impl From<std::io::Error> for InstallerError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for InstallerError {
    fn from(err: serde_json::Error) -> Self {
        Self::JsonParse(err.to_string())
    }
}

impl Serialize for InstallerError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
