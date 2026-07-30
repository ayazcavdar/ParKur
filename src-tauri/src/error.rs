use crate::i18n;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Serialize)]
pub struct ClientError {
    pub code: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, String>,
}

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
    Network(String),
}

impl InstallerError {
    pub fn coded(variant: fn(String) -> Self, code: &str, params: &[(&str, &str)]) -> Self {
        variant(i18n::encode(code, params))
    }

    pub fn to_client_error(&self) -> ClientError {
        let raw = match self {
            Self::DiskOperation(m)
            | Self::IsoExtraction(m)
            | Self::BootloaderConfig(m)
            | Self::PermissionDenied(m)
            | Self::CommandExecution(m)
            | Self::InvalidInput(m)
            | Self::Io(m)
            | Self::JsonParse(m)
            | Self::ImageOperation(m)
            | Self::InitramfsBuild(m)
            | Self::Network(m) => m,
        };
        let (code, params) = if raw.starts_with("ERR_") || raw.starts_with("progress.") {
            i18n::decode(raw)
        } else {
            (
                "ERR_GENERIC".to_string(),
                HashMap::from([("detail".to_string(), raw.clone())]),
            )
        };
        ClientError { code, params }
    }
}

impl fmt::Display for InstallerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.to_client_error();
        if c.params.is_empty() {
            write!(f, "{}", c.code)
        } else {
            write!(f, "{}|{:?}", c.code, c.params)
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
        self.to_client_error().serialize(serializer)
    }
}
