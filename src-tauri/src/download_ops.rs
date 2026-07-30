use crate::error::InstallerError;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

const ISO_BASE: &str = "https://indir.pardus.org.tr/ISO/Pardus25";
const SHA256SUMS_URL: &str = "https://indir.pardus.org.tr/ISO/Pardus25/SHA256SUMS";

static CANCEL_DOWNLOAD: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoCatalogEntry {
    pub id: String,
    pub version: String,
    pub desktop: String,
    pub filename: String,
    pub url: String,
    pub size_hint_gb: f64,
}

#[derive(Clone, Serialize)]
pub struct DownloadProgressPayload {
    pub phase: String,
    pub percent: u32,
    pub downloaded_mb: u64,
    pub total_mb: u64,
    pub path: String,
}

fn entry(id: &str, version: &str, desktop: &str, filename: &str) -> IsoCatalogEntry {
    IsoCatalogEntry {
        id: id.to_string(),
        version: version.to_string(),
        desktop: desktop.to_string(),
        filename: filename.to_string(),
        url: format!("{ISO_BASE}/{filename}"),
        size_hint_gb: 3.0,
    }
}

/// Fixed catalog: latest Pardus 25.x desktop editions only.
pub fn iso_catalog() -> Vec<IsoCatalogEntry> {
    vec![
        entry(
            "pardus-25.2-xfce",
            "25.2",
            "XFCE",
            "Pardus-25.2-XFCE-amd64.iso",
        ),
        entry(
            "pardus-25.2-gnome",
            "25.2",
            "GNOME",
            "Pardus-25.2-GNOME-amd64.iso",
        ),
    ]
}

pub fn find_catalog_entry(id: &str) -> Result<IsoCatalogEntry, InstallerError> {
    iso_catalog()
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| {
            InstallerError::coded(
                InstallerError::InvalidInput,
                "ERR_ISO_UNKNOWN_ID",
                &[("id", id)],
            )
        })
}

pub fn download_dir() -> Result<PathBuf, InstallerError> {
    let home = std::env::var_os("USERPROFILE").ok_or_else(|| {
        InstallerError::coded(InstallerError::Io, "ERR_ISO_DOWNLOAD_DIR", &[])
    })?;
    let dir = PathBuf::from(home).join("Downloads").join("ParKur");
    std::fs::create_dir_all(&dir).map_err(|e| {
        InstallerError::Io(format!("create download dir {}: {e}", dir.display()))
    })?;
    Ok(dir)
}

pub fn request_cancel_download() {
    CANCEL_DOWNLOAD.store(true, Ordering::SeqCst);
}

fn emit_progress(
    app: &AppHandle,
    phase: &str,
    percent: u32,
    downloaded_mb: u64,
    total_mb: u64,
    path: &str,
) {
    let _ = app.emit(
        "iso-download-progress",
        DownloadProgressPayload {
            phase: phase.to_string(),
            percent,
            downloaded_mb,
            total_mb,
            path: path.to_string(),
        },
    );
}

async fn fetch_expected_sha256(filename: &str) -> Result<String, InstallerError> {
    let client = http_client()?;
    let text = client
        .get(SHA256SUMS_URL)
        .send()
        .await
        .map_err(|e| {
            InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_CHECKSUM_FETCH",
                &[("detail", &e.to_string())],
            )
        })?
        .error_for_status()
        .map_err(|e| {
            InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_CHECKSUM_FETCH",
                &[("detail", &e.to_string())],
            )
        })?
        .text()
        .await
        .map_err(|e| {
            InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_CHECKSUM_FETCH",
                &[("detail", &e.to_string())],
            )
        })?;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hash = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("").trim_start_matches('*');
        if name == filename && hash.len() == 64 {
            return Ok(hash.to_lowercase());
        }
    }

    Err(InstallerError::coded(
        InstallerError::Network,
        "ERR_ISO_CHECKSUM_MISSING",
        &[("file", filename)],
    ))
}

fn http_client() -> Result<reqwest::Client, InstallerError> {
    reqwest::Client::builder()
        .user_agent(concat!("ParKur/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| InstallerError::Network(format!("http client: {e}")))
}

async fn sha256_file(path: &Path) -> Result<String, InstallerError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| InstallerError::Io(format!("open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf)
            .await
            .map_err(|e| InstallerError::Io(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub async fn download_iso(app: AppHandle, id: String) -> Result<String, InstallerError> {
    if DOWNLOAD_BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(InstallerError::coded(
            InstallerError::InvalidInput,
            "ERR_ISO_DOWNLOAD_BUSY",
            &[],
        ));
    }

    let result = download_iso_inner(app, id).await;
    DOWNLOAD_BUSY.store(false, Ordering::SeqCst);
    result
}

async fn download_iso_inner(app: AppHandle, id: String) -> Result<String, InstallerError> {
    CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);
    let entry = find_catalog_entry(&id)?;
    let dir = download_dir()?;
    let dest = dir.join(&entry.filename);
    let dest_str = dest.to_string_lossy().to_string();
    let partial = dir.join(format!("{}.partial", entry.filename));

    emit_progress(&app, "preparing", 0, 0, 0, &dest_str);

    let expected = fetch_expected_sha256(&entry.filename).await?;

    if dest.is_file() {
        emit_progress(&app, "verifying", 0, 0, 0, &dest_str);
        let existing = sha256_file(&dest).await?;
        if existing.eq_ignore_ascii_case(&expected) {
            emit_progress(&app, "done", 100, 0, 0, &dest_str);
            return Ok(dest_str);
        }
        let _ = tokio::fs::remove_file(&dest).await;
    }

    if CANCEL_DOWNLOAD.load(Ordering::SeqCst) {
        return Err(InstallerError::coded(
            InstallerError::Network,
            "ERR_ISO_DOWNLOAD_CANCELLED",
            &[],
        ));
    }

    let client = http_client()?;
    let response = client
        .get(&entry.url)
        .send()
        .await
        .map_err(|e| {
            InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_DOWNLOAD_FAILED",
                &[("detail", &e.to_string())],
            )
        })?
        .error_for_status()
        .map_err(|e| {
            InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_DOWNLOAD_FAILED",
                &[("detail", &e.to_string())],
            )
        })?;

    let total = response.content_length().unwrap_or(0);
    let total_mb = total / (1024 * 1024);
    let mut file = tokio::fs::File::create(&partial).await.map_err(|e| {
        InstallerError::Io(format!("create {}: {e}", partial.display()))
    })?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emit = 0u64;

    while let Some(chunk) = stream.next().await {
        if CANCEL_DOWNLOAD.load(Ordering::SeqCst) {
            drop(file);
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_DOWNLOAD_CANCELLED",
                &[],
            ));
        }

        let chunk = chunk.map_err(|e| {
            InstallerError::coded(
                InstallerError::Network,
                "ERR_ISO_DOWNLOAD_FAILED",
                &[("detail", &e.to_string())],
            )
        })?;
        file.write_all(&chunk).await.map_err(|e| {
            InstallerError::Io(format!("write {}: {e}", partial.display()))
        })?;
        downloaded += chunk.len() as u64;

        if downloaded - last_emit >= 8 * 1024 * 1024 || (total > 0 && downloaded == total) {
            last_emit = downloaded;
            let percent = if total > 0 {
                ((downloaded as f64 / total as f64) * 100.0).min(99.0) as u32
            } else {
                0
            };
            emit_progress(
                &app,
                "downloading",
                percent,
                downloaded / (1024 * 1024),
                total_mb,
                &dest_str,
            );
        }
    }

    file.flush().await.map_err(|e| {
        InstallerError::Io(format!("flush {}: {e}", partial.display()))
    })?;
    drop(file);

    emit_progress(
        &app,
        "verifying",
        99,
        downloaded / (1024 * 1024),
        total_mb,
        &dest_str,
    );
    let actual = sha256_file(&partial).await?;
    if !actual.eq_ignore_ascii_case(&expected) {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(InstallerError::coded(
            InstallerError::Network,
            "ERR_ISO_CHECKSUM_MISMATCH",
            &[("file", &entry.filename)],
        ));
    }

    tokio::fs::rename(&partial, &dest).await.map_err(|e| {
        InstallerError::Io(format!(
            "rename {} -> {}: {e}",
            partial.display(),
            dest.display()
        ))
    })?;

    emit_progress(
        &app,
        "done",
        100,
        downloaded / (1024 * 1024),
        total_mb,
        &dest_str,
    );
    Ok(dest_str)
}
