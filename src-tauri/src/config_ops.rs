use crate::disk_ops::run_powershell;
use crate::error::InstallerError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub username: String,
    pub password: String, // düz metin — yalnızca RAM'de, diske hash olarak yazılır
    pub hostname: String,
    pub locale: String,
    pub timezone: String,
    pub keyboard: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParkurConf {
    version: u32,
    username: String,
    password_hash: String,
    hostname: String,
    locale: String,
    timezone: String,
    keyboard: String,
    target_label: String,
}

/// SHA-512 crypt hash üretir ($6$...). Linux /etc/shadow formatı.
fn hash_password(password: &str) -> Result<String, InstallerError> {
    use sha_crypt::{sha512_simple, Sha512Params};

    let params = Sha512Params::new(5000)
        .map_err(|e| InstallerError::InvalidInput(format!("SHA-512 param hatası: {:?}", e)))?;

    sha512_simple(password, &params)
        .map_err(|e| InstallerError::InvalidInput(format!("SHA-512 hash hatası: {:?}", e)))
}

/// Kullanıcı bilgilerini parkur.conf olarak hedef bölüme yazar.
/// Şifre SHA-512 crypt hash'e dönüştürülür — düz metin diske asla yazılmaz.
pub fn write_parkur_conf(
    target_letter: &str,
    config: &UserConfig,
) -> Result<(), InstallerError> {
    let password_hash = hash_password(&config.password)?;

    let conf = ParkurConf {
        version: 1,
        username: config.username.clone(),
        password_hash,
        hostname: config.hostname.clone(),
        locale: config.locale.clone(),
        timezone: config.timezone.clone(),
        keyboard: config.keyboard.clone(),
        target_label: "NextOS_Install".to_string(),
    };

    let json = serde_json::to_string_pretty(&conf)?;
    let conf_path = format!("{}:\\parkur.conf", target_letter);

    // PowerShell ile UTF-8 yazma
    let escaped = json.replace("'", "''");
    let script = format!(
        "Set-Content -Path '{}' -Value '{}' -Encoding UTF8 -Force -ErrorAction Stop",
        conf_path, escaped
    );
    run_powershell(&script).map_err(|e| {
        InstallerError::Io(format!("parkur.conf yazılamadı: {}", e))
    })?;

    println!("[CONFIG] parkur.conf yazıldı: {}", conf_path);
    Ok(())
}

/// Kullanıcı girdilerini doğrular.
pub fn validate_user_config(config: &UserConfig) -> Result<(), InstallerError> {
    if config.username.is_empty() || config.username.len() > 32 {
        return Err(InstallerError::InvalidInput(
            "Kullanıcı adı 1-32 karakter olmalı.".into(),
        ));
    }
    // Linux kullanıcı adı kuralları: küçük harf, rakam, tire, alt çizgi
    if !config
        .username
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(InstallerError::InvalidInput(
            "Kullanıcı adı yalnızca küçük harf, rakam, tire ve alt çizgi içerebilir.".into(),
        ));
    }
    if !config.username.chars().next().unwrap_or('0').is_ascii_lowercase() {
        return Err(InstallerError::InvalidInput(
            "Kullanıcı adı bir küçük harf ile başlamalı.".into(),
        ));
    }
    if config.password.len() < 4 {
        return Err(InstallerError::InvalidInput(
            "Şifre en az 4 karakter olmalı.".into(),
        ));
    }
    if config.hostname.is_empty() {
        return Err(InstallerError::InvalidInput(
            "Bilgisayar adı boş olamaz.".into(),
        ));
    }
    Ok(())
}
