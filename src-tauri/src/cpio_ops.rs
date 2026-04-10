// =============================================================================
// ParKur — CPIO Newc Arşiv Üretici + Supplementary Initrd Builder
// =============================================================================
// Linux çekirdeğinin çoklu initrd desteğinden yararlanarak, ISO'yu hiç
// değiştirmeden dosya enjekte eder. cpio newc formatı + gzip sıkıştırma.
// =============================================================================

use crate::error::InstallerError;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

/// cpio newc arşivine bir dosya girişi yazar.
fn cpio_write_entry(archive: &mut Vec<u8>, path: &str, data: &[u8], mode: u32) {
    let namesize = path.len() + 1; // null terminator
    let filesize = data.len();

    // cpio newc header: 110 byte sabit + dosya adı + padding
    let header = format!(
        "070701\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}\
         {:08X}",
        0,        // ino
        mode,     // mode
        0,        // uid (root)
        0,        // gid (root)
        1,        // nlink
        0,        // mtime
        filesize, // filesize
        0,        // devmajor
        0,        // devminor
        0,        // rdevmajor
        0,        // rdevminor
        namesize, // namesize
        0,        // check
    );

    archive.extend_from_slice(header.as_bytes());
    archive.extend_from_slice(path.as_bytes());
    archive.push(0); // null terminator

    // header + name 4-byte hizalama
    let header_total = 110 + namesize;
    let pad = (4 - (header_total % 4)) % 4;
    archive.extend(std::iter::repeat(0u8).take(pad));

    // dosya verisi
    archive.extend_from_slice(data);

    // veri 4-byte hizalama
    let data_pad = (4 - (filesize % 4)) % 4;
    archive.extend(std::iter::repeat(0u8).take(data_pad));
}

/// cpio arşivine dizin girişi yazar.
fn cpio_write_dir(archive: &mut Vec<u8>, path: &str) {
    cpio_write_entry(archive, path, &[], 0o040755);
}

/// cpio arşivi sonlandırıcı (TRAILER!!!) yazar.
fn cpio_write_trailer(archive: &mut Vec<u8>) {
    cpio_write_entry(archive, "TRAILER!!!", &[], 0);
    // 512 byte blok hizalama
    let pad = (512 - (archive.len() % 512)) % 512;
    archive.extend(std::iter::repeat(0u8).take(pad));
}

/// parkur-engine.sh kurulum motoru script içeriği.
fn engine_script() -> &'static str {
    include_str!("../scripts/parkur-engine.sh")
}

/// live-bottom hook script içeriği (initramfs → rootfs enjeksiyon).
fn live_bottom_hook() -> &'static str {
    include_str!("../scripts/99-parkur")
}

/// systemd unit dosyası içeriği.
fn systemd_service() -> &'static str {
    include_str!("../scripts/parkur.service")
}

/// Supplementary initrd imajı oluşturur (cpio newc + gzip).
/// Dönen Vec<u8> doğrudan .img dosyası olarak yazılabilir.
pub fn build_supplementary_initrd() -> Result<Vec<u8>, InstallerError> {
    let mut archive: Vec<u8> = Vec::with_capacity(32768);

    // Dizin yapısı
    cpio_write_dir(&mut archive, "scripts");
    cpio_write_dir(&mut archive, "scripts/live-bottom");
    cpio_write_dir(&mut archive, "parkur-payload");

    // Dosyalar
    cpio_write_entry(
        &mut archive,
        "scripts/live-bottom/99-parkur",
        live_bottom_hook().as_bytes(),
        0o100755, // rwxr-xr-x
    );

    cpio_write_entry(
        &mut archive,
        "parkur-payload/parkur.service",
        systemd_service().as_bytes(),
        0o100644,
    );

    cpio_write_entry(
        &mut archive,
        "parkur-payload/parkur-engine.sh",
        engine_script().as_bytes(),
        0o100755,
    );

    cpio_write_trailer(&mut archive);

    // gzip sıkıştırma
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&archive).map_err(|e| {
        InstallerError::Io(format!("gzip sıkıştırma hatası: {}", e))
    })?;

    let compressed = encoder.finish().map_err(|e| {
        InstallerError::Io(format!("gzip finalize hatası: {}", e))
    })?;

    println!(
        "[CPIO] Supplementary initrd oluşturuldu: {} byte (sıkıştırılmış: {} byte)",
        archive.len(),
        compressed.len()
    );

    Ok(compressed)
}

/// Supplementary initrd'yi hedef bölüme yazar.
pub fn write_initrd_to_partition(target_letter: &str) -> Result<(), InstallerError> {
    let data = build_supplementary_initrd()?;
    let path = format!("{}:\\parkur-hook.img", target_letter);

    std::fs::write(&path, &data).map_err(|e| {
        InstallerError::Io(format!("parkur-hook.img yazılamadı: {}", e))
    })?;

    println!("[CPIO] parkur-hook.img yazıldı: {} ({} byte)", path, data.len());
    Ok(())
}
