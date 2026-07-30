use crate::error::InstallerError;
use crate::squashfs_ops::{module_parent_dirs, NtfsModuleBlob};
use crate::util::to_lf;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::Path;

const OVERLAY_INIT: &str = include_str!("../overlay-template/init");
const OVERLAY_FIRSTBOOT: &str = include_str!("../overlay-template/usr/local/sbin/nextos-firstboot");

struct OverlayEntry {
    path: String,
    mode: u32,
    content: Vec<u8>,
}

const MODE_DIR: u32 = 0o040755;
const MODE_EXEC: u32 = 0o100755;
const MODE_FILE: u32 = 0o100644;
const MODE_FILE_SECRET: u32 = 0o100600;

/// Build the standalone NextOS overlay initrd (gzipped cpio newc archive).
#[allow(dead_code)]
pub fn build_overlay_cpio_gz(dest: &Path, install_password: &str) -> Result<(), InstallerError> {
    build_overlay_cpio_gz_with_extras(dest, Some(install_password), &[])
}

pub fn build_overlay_cpio_gz_with_extras(
    dest: &Path,
    install_password: Option<&str>,
    extras: &[NtfsModuleBlob],
) -> Result<(), InstallerError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            InstallerError::InitramfsBuild(format!("overlay dest dir create failed: {}", e))
        })?;
    }
    let entries = stage_overlay_entries(install_password, extras);
    let cpio_raw = build_cpio_newc(&entries)?;
    gzip_native(&cpio_raw, dest)
}

#[allow(dead_code)]
pub fn build_overlay_cpio_gz_without_secrets(dest: &Path) -> Result<(), InstallerError> {
    build_overlay_cpio_gz_with_extras(dest, None, &[])
}

fn stage_overlay_entries(
    install_password: Option<&str>,
    extras: &[NtfsModuleBlob],
) -> Vec<OverlayEntry> {
    let mut v: Vec<OverlayEntry> = Vec::new();
    let mut seen_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    let push_dir = |v: &mut Vec<OverlayEntry>, seen: &mut std::collections::HashSet<String>, d: &str| {
        if seen.insert(d.to_string()) {
            v.push(OverlayEntry {
                path: d.to_string(),
                mode: MODE_DIR,
                content: Vec::new(),
            });
        }
    };

    for d in &[
        "usr",
        "usr/local",
        "usr/local/sbin",
        "var",
        "var/lib",
        "var/lib/nextos",
    ] {
        push_dir(&mut v, &mut seen_dirs, d);
    }

    if let Some(pass) = install_password {
        let pass_clean: String = pass
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();
        v.push(OverlayEntry {
            path: "var/lib/nextos/install.pass".into(),
            mode: MODE_FILE_SECRET,
            content: pass_clean.into_bytes(),
        });
    }

    v.push(OverlayEntry {
        path: "init".into(),
        mode: MODE_EXEC,
        content: to_lf(OVERLAY_INIT).into_bytes(),
    });

    v.push(OverlayEntry {
        path: "usr/local/sbin/nextos-firstboot".into(),
        mode: MODE_EXEC,
        content: to_lf(OVERLAY_FIRSTBOOT).into_bytes(),
    });

    for extra in extras {
        for d in module_parent_dirs(&extra.cpio_path) {
            push_dir(&mut v, &mut seen_dirs, &d);
        }
        v.push(OverlayEntry {
            path: extra.cpio_path.clone(),
            mode: MODE_FILE,
            content: extra.data.clone(),
        });
    }

    v
}

fn build_cpio_newc(entries: &[OverlayEntry]) -> Result<Vec<u8>, InstallerError> {
    let mut out: Vec<u8> = Vec::new();
    let mut ino: u64 = 1;
    for entry in entries {
        let is_dir = (entry.mode & 0o170000) == 0o040000;
        let data: &[u8] = if is_dir { &[] } else { &entry.content };
        write_newc_record(&mut out, ino, entry.mode, &entry.path, data)?;
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
