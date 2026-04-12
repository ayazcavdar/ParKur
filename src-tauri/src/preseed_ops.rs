use crate::error::InstallerError;
use std::io::Write;

/// CRLF → LF dönüşümü. Linux'a giden hiçbir dosyada \r karakteri olmayacak.
pub fn to_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// persistence.conf içeriği üretir.
pub fn generate_persistence_conf() -> String {
    to_lf("/ union\n")
}

/// install-hook.sh içeriği üretir. Kullanıcı oluşturma komutlarını içerir.
pub fn generate_install_hook(username: &str, password: &str) -> String {
    let script = format!(
        r#"#!/bin/bash
# NextOS Auto User Setup - Otomatik kullanici olusturma
set -e

# Kullanici olustur
useradd -m -s /bin/bash {username}

# Sifre ata
chpasswd <<'NEXTOS_PASSWD'
{username}:{password}
NEXTOS_PASSWD

# sudo grubuna ekle
usermod -aG sudo {username}

echo "Kullanici '{username}' basariyla olusturuldu."
"#,
        username = username,
        password = password,
    );
    to_lf(&script)
}

/// Linux hedef dosyasını CRLF→LF dönüşümü ile diske yazar. BOM eklenmez.
pub fn write_linux_file(path: &str, content: &str) -> Result<(), InstallerError> {
    let lf_content = to_lf(content);

    // Üst dizini oluştur (yoksa)
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| InstallerError::Io(format!("Dizin oluşturulamadı ({}): {}", path, e)))?;
    }

    let mut file = std::fs::File::create(path)
        .map_err(|e| InstallerError::Io(format!("{} oluşturulamadı: {}", path, e)))?;
    file.write_all(lf_content.as_bytes())
        .map_err(|e| InstallerError::Io(format!("{} yazılamadı: {}", path, e)))?;

    println!("[PRESEED] Dosya yazıldı: {}", path);
    Ok(())
}
