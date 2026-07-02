use crate::error::InstallerError;
use crate::util::to_lf;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::Path;

const OVERLAY_INIT: &str = include_str!("../overlay-template/init");
const OVERLAY_FIRSTBOOT: &str = include_str!("../overlay-template/usr/local/sbin/nextos-firstboot");

struct OverlayEntry {
    path: &'static str,
    mode: u32,
    content: String,
}

const MODE_DIR: u32 = 0o040755;
const MODE_EXEC: u32 = 0o100755;

/// Build the standalone NextOS overlay initrd (gzipped cpio newc archive).
///
/// GRUB loads this as a SECOND initrd after the stock ISO initrd
/// (`initrd /NextOS/boot/initrd.img /NextOS/boot/overlay.cpio.gz`).
/// The kernel extracts both archives in order, so files here (notably
/// `/init`) replace their counterparts from the stock live-boot initrd.
/// Keeping the overlay separate avoids fragile manual concatenation with
/// the (zstd-compressed) stock initrd and lets us iterate on the overlay
/// without touching the big stock image.
pub fn build_overlay_cpio_gz(dest: &Path) -> Result<(), InstallerError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            InstallerError::InitramfsBuild(format!("overlay dest dir create failed: {}", e))
        })?;
    }
    let entries = stage_overlay_entries();
    let cpio_raw = build_cpio_newc(&entries)?;
    gzip_native(&cpio_raw, dest)
}

fn stage_overlay_entries() -> Vec<OverlayEntry> {
    let mut v: Vec<OverlayEntry> = Vec::new();

    // Directory entries (must come before files inside them)
    for d in &["usr", "usr/local", "usr/local/sbin"] {
        v.push(OverlayEntry {
            path: d,
            mode: MODE_DIR,
            content: String::new(),
        });
    }

    // /init — overwrites the live-boot init in the stock Pardus initrd so that
    // our loop-mount logic runs instead of live-boot's CD/USB discovery.
    v.push(OverlayEntry {
        path: "init",
        mode: MODE_EXEC,
        content: to_lf(OVERLAY_INIT),
    });

    // nextos-firstboot — staged into the new root during first-boot bootstrap.
    v.push(OverlayEntry {
        path: "usr/local/sbin/nextos-firstboot",
        mode: MODE_EXEC,
        content: to_lf(OVERLAY_FIRSTBOOT),
    });

    v
}

fn build_cpio_newc(entries: &[OverlayEntry]) -> Result<Vec<u8>, InstallerError> {
    let mut out: Vec<u8> = Vec::new();
    let mut ino: u64 = 1;
    for entry in entries {
        let is_dir = (entry.mode & 0o170000) == 0o040000;
        let data: &[u8] = if is_dir {
            &[]
        } else {
            entry.content.as_bytes()
        };
        write_newc_record(&mut out, ino, entry.mode, entry.path, data)?;
        ino += 1;
    }
    write_newc_record(&mut out, ino, 0, "TRAILER!!!", &[])?;
    while out.len() % 512 != 0 {
        out.push(0);
    }
    Ok(out)
}

fn write_newc_record(
    out: &mut Vec<u8>,
    ino: u64,
    mode: u32,
    name: &str,
    data: &[u8],
) -> Result<(), InstallerError> {
    let name_bytes = name.as_bytes();
    let namesize = (name_bytes.len() + 1) as u32;
    let filesize = data.len() as u32;
    let mtime: u32 = 0;
    let nlink: u32 = if (mode & 0o170000) == 0o040000 { 2 } else { 1 };

    let header = format!(
        "070701{ino:08x}{mode:08x}{uid:08x}{gid:08x}{nlink:08x}{mtime:08x}{filesize:08x}{devmaj:08x}{devmin:08x}{rmaj:08x}{rmin:08x}{namesize:08x}{check:08x}",
        ino = (ino as u32),
        mode = mode,
        uid = 0u32,
        gid = 0u32,
        nlink = nlink,
        mtime = mtime,
        filesize = filesize,
        devmaj = 0u32,
        devmin = 0u32,
        rmaj = 0u32,
        rmin = 0u32,
        namesize = namesize,
        check = 0u32,
    );

    if header.len() != 110 {
        return Err(InstallerError::InitramfsBuild(format!(
            "cpio header length invalid: {}",
            header.len()
        )));
    }

    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(name_bytes);
    out.push(0);
    align4(out);

    out.extend_from_slice(data);
    align4(out);

    Ok(())
}

fn align4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn gzip_native(raw: &[u8], dest: &Path) -> Result<(), InstallerError> {
    let mut buf: Vec<u8> = Vec::with_capacity(raw.len() / 3 + 1024);
    {
        let mut enc = GzEncoder::new(&mut buf, Compression::best());
        enc.write_all(raw).map_err(|e| {
            InstallerError::InitramfsBuild(format!("gzip native write failed: {}", e))
        })?;
        enc.finish().map_err(|e| {
            InstallerError::InitramfsBuild(format!("gzip native finish failed: {}", e))
        })?;
    }
    std::fs::write(dest, &buf).map_err(|e| {
        InstallerError::InitramfsBuild(format!("gzip native output write failed: {}", e))
    })?;
    Ok(())
}
