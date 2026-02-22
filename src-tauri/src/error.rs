// =============================================================================
// Next OS Installer - Hata Yönetim Modülü
// =============================================================================
// Tüm modüller bu merkezi hata tipini kullanır.
// Memory Safety: Rust'ın ownership sistemi sayesinde hata durumlarında bile
// bellek sızıntısı veya dangling pointer oluşmaz.
// =============================================================================

use serde::Serialize;
use std::fmt;

/// Ana hata tipi - Tüm installer operasyonlarının dönüş tipi
#[derive(Debug)]
pub enum InstallerError {
    /// Disk bölümleme/küçültme/formatlama hatası
    DiskOperation(String),
    /// ISO montaj/çıkartma hatası
    IsoExtraction(String),
    /// Bootloader (bcdedit) yapılandırma hatası
    BootloaderConfig(String),
    /// Yönetici yetkisi eksik (diskpart/bcdedit için zorunlu)
    PermissionDenied(String),
    /// Harici komut çalıştırma hatası (PowerShell, diskpart, bcdedit)
    CommandExecution(String),
    /// Geçersiz kullanıcı girdisi
    InvalidInput(String),
    /// Dosya sistemi G/Ç hatası
    Io(String),
    /// JSON ayrıştırma hatası (PowerShell çıktıları için)
    JsonParse(String),
}

impl fmt::Display for InstallerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiskOperation(msg) => write!(f, "Disk işlemi hatası: {}", msg),
            Self::IsoExtraction(msg) => write!(f, "ISO çıkartma hatası: {}", msg),
            Self::BootloaderConfig(msg) => write!(f, "Bootloader hatası: {}", msg),
            Self::PermissionDenied(msg) => write!(f, "Yetki hatası: {}", msg),
            Self::CommandExecution(msg) => write!(f, "Komut çalıştırma hatası: {}", msg),
            Self::InvalidInput(msg) => write!(f, "Geçersiz giriş: {}", msg),
            Self::Io(msg) => write!(f, "G/Ç hatası: {}", msg),
            Self::JsonParse(msg) => write!(f, "JSON ayrıştırma hatası: {}", msg),
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

// Tauri v2 hata tiplerinin serileştirilebilir olmasını zorunlu kılar.
// Bu impl sayesinde hatalar frontend'e JSON string olarak iletilir.
impl Serialize for InstallerError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
