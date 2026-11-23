//! What the file manager does to an FTP session's files: the counterparts
//! of `sftp::browser` and `sftp::transfer`, with the same events for a
//! transfer's progress, end and failure.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::client::{Client, FtpError, DATA_IDLE};
use super::list::{self, base_name, parent};
use super::{with_client, FtpConn};
use crate::sftp::browser::RemoteEntry;
use crate::sftp::local_download::LocalDownload;
use crate::sftp::transfer::plan_directory;

/// Hears a transfer's bytes done so far and its total.
pub type Progress<'a> = &'a (dyn Fn(u64, u64) + Send + Sync);

/// Largest file opened in the editor or viewer, as for SFTP.
const MAX_EDIT_SIZE: u64 = 5 * 1024 * 1024;
const CHUNK: usize = 256 * 1024;
/// A folder tree deeper than this is not deleted.
const DELETE_DEPTH: usize = 32;

type Boxed<'a, T> = Pin<Box<dyn Future<Output = Result<T, FtpError>> + Send + 'a>>;

fn sort(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

/// After sending a file: wait for the server to close its side. Some
/// (vsftpd) answer the TLS goodbye with their own, and a socket closed
/// before that arrives is reset, which they report as a failed upload.
async fn drain(s: &mut Box<dyn super::client::Stream>) {
    let mut sink = [0u8; 512];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Ok(n) = s.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
}

/// A server close without TLS's goodbye (close_notify) still ends a transfer
/// that has arrived whole; many servers skip it.
fn ended(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::UnexpectedEof
}

async fn list_on(c: &mut Client, path: &str) -> Result<Vec<RemoteEntry>, FtpError> {
    let (text, mlsd) = c.list_text(path).await?;
    let mut entries = if mlsd { list::parse_mlsd(&text, path) } else { list::parse_list(&text, path) };
    sort(&mut entries);
    Ok(entries)
}

pub async fn list_dir(conn: &FtpConn, path: &str) -> Result<Vec<RemoteEntry>, FtpError> {
    with_client!(conn, |c| list_on(c, path))
}

async fn is_dir(c: &mut Client, path: &str) -> Result<bool, FtpError> {
    Ok(c.cmd(&format!("CWD {path}")).await?.code == 250)
}

/// Whether the folder `path` is in has an entry of its name.
async fn exists(c: &mut Client, path: &str) -> Result<bool, FtpError> {
    let name = base_name(path).to_string();
    match list_on(c, &parent(path)).await {
        Ok(entries) => Ok(entries.iter().any(|e| e.name == name)),
        Err(FtpError::Reply { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `mkdir -p`: each missing folder on the way, from the top.
async fn mkdirs(c: &mut Client, path: &str) -> Result<(), FtpError> {
    let mut missing = Vec::new();
    let mut at = path.trim_end_matches('/').to_string();
    while !at.is_empty() && at != "/" && !is_dir(c, &at).await? {
        missing.push(at.clone());
        at = parent(&at);
    }
    for dir in missing.into_iter().rev() {
        let r = c.cmd(&format!("MKD {dir}")).await?;
        if r.code != 257 && !is_dir(c, &dir).await? {
            return Err(FtpError::Reply { code: r.code, text: format!("{dir}: {}", r.text) });
        }
    }
    Ok(())
}

pub async fn make_dir(conn: &FtpConn, path: &str) -> Result<(), FtpError> {
    with_client!(conn, |c| mkdirs(c, path))
}

fn delete_on<'a>(c: &'a mut Client, path: &'a str, depth: usize) -> Boxed<'a, ()> {
    Box::pin(async move {
        let r = c.cmd(&format!("DELE {path}")).await?;
        if r.code == 250 {
            return Ok(());
        }
        if !is_dir(c, path).await? {
            return Err(FtpError::Reply { code: r.code, text: format!("{path}: {}", r.text) });
        }
        if depth >= DELETE_DEPTH {
            return Err(FtpError::Other(format!("{path}: folders nested too deep to delete")));
        }
        for e in list_on(c, path).await? {
            delete_on(c, &e.path, depth + 1).await?;
        }
        // Out of it first: some servers will not remove the folder one is in.
        let _ = c.cmd(&format!("CWD {}", parent(path))).await?;
        let r = c.cmd(&format!("RMD {path}")).await?;
        if r.code != 250 {
            return Err(FtpError::Reply { code: r.code, text: format!("{path}: {}", r.text) });
        }
        Ok(())
    })
}

pub async fn delete(conn: &FtpConn, path: &str) -> Result<(), FtpError> {
    with_client!(conn, |c| delete_on(c, path, 0))
}

fn already_exists(path: &str) -> FtpError {
    FtpError::Other(format!("{path}: a file or folder of that name already exists"))
}

async fn rename_on(c: &mut Client, from: &str, to: &str) -> Result<(), FtpError> {
    if exists(c, to).await? {
        return Err(already_exists(to));
    }
    c.ok(&format!("RNFR {from}"), &[350]).await?;
    c.ok(&format!("RNTO {to}"), &[250]).await?;
    Ok(())
}

pub async fn rename(conn: &FtpConn, from: &str, to: &str) -> Result<(), FtpError> {
    with_client!(conn, |c| rename_on(c, from, to))
}

/// Write `data` to `path`, replacing what is there.
async fn store_bytes(c: &mut Client, path: &str, data: &[u8]) -> Result<(), FtpError> {
    let mut s = c.data(&format!("STOR {path}")).await?;
    let sent = async {
        s.write_all(data).await?;
        s.shutdown().await
    };
    let sent = tokio::time::timeout(DATA_IDLE, sent).await;
    if matches!(sent, Ok(Ok(()))) {
        drain(&mut s).await;
    }
    drop(s);
    match sent {
        Ok(Ok(())) => c.finish().await,
        Ok(Err(e)) => {
            c.broken = true;
            Err(FtpError::Io(format!("{path}: {e}")))
        }
        Err(_) => {
            c.broken = true;
            Err(FtpError::Io(format!("{path}: the server stopped taking the data")))
        }
    }
}

async fn touch_on(c: &mut Client, path: &str) -> Result<(), FtpError> {
    if exists(c, path).await? {
        return Ok(());
    }
    store_bytes(c, path, &[]).await
}

pub async fn touch(conn: &FtpConn, path: &str) -> Result<(), FtpError> {
    with_client!(conn, |c| touch_on(c, path))
}

async fn size_of(c: &mut Client, path: &str) -> Result<Option<u64>, FtpError> {
    let r = c.cmd(&format!("SIZE {path}")).await?;
    Ok((r.code == 213).then(|| r.text.trim().parse().ok()).flatten())
}

async fn read_on(c: &mut Client, path: &str) -> Result<String, FtpError> {
    let too_large = |size: u64| {
        FtpError::Other(format!(
            "File too large to edit ({:.1} MB, max {:.0} MB)",
            size as f64 / (1024.0 * 1024.0),
            MAX_EDIT_SIZE as f64 / (1024.0 * 1024.0)
        ))
    };
    if let Some(size) = size_of(c, path).await? {
        if size > MAX_EDIT_SIZE {
            return Err(too_large(size));
        }
    }
    let s = c.data(&format!("RETR {path}")).await?;
    let mut out = Vec::new();
    let read = tokio::time::timeout(DATA_IDLE, s.take(MAX_EDIT_SIZE + 1).read_to_end(&mut out)).await;
    match read {
        Ok(Ok(_)) => {}
        Ok(Err(e)) if ended(&e) => {}
        Ok(Err(e)) => {
            c.broken = true;
            return Err(FtpError::Io(format!("{path}: {e}")));
        }
        Err(_) => {
            c.broken = true;
            return Err(FtpError::Io(format!("{path}: the server stopped sending")));
        }
    }
    if out.len() as u64 > MAX_EDIT_SIZE {
        // Cut short: the server's answer is not worth waiting for.
        c.broken = true;
        return Err(too_large(out.len() as u64));
    }
    c.finish().await?;
    String::from_utf8(out).map_err(|_| FtpError::Other("File is not valid UTF-8 text".into()))
}

pub async fn read_text(conn: &FtpConn, path: &str) -> Result<String, FtpError> {
    with_client!(conn, |c| read_on(c, path))
}

pub async fn write_text(conn: &FtpConn, path: &str, content: &str) -> Result<(), FtpError> {
    with_client!(conn, |c| store_bytes(c, path, content.as_bytes()))
}

/// Download one file into `local`, through a staging file published only
/// when the whole file is there.
async fn download_on(
    c: &mut Client,
    remote: &str,
    local: &str,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> Result<u64, FtpError> {
    let size = size_of(c, remote).await?;
    let mut file = LocalDownload::new(Path::new(local)).map_err(|e| FtpError::Other(e.to_string()))?;
    let mut s = c.data(&format!("RETR {remote}")).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut done: u64 = 0;
    let mut shown: u64 = 0;
    progress(0, size.unwrap_or(0));
    loop {
        let n = tokio::select! {
            _ = cancel.cancelled() => {
                drop(s);
                c.broken = true;
                return Err(FtpError::Cancelled);
            }
            r = tokio::time::timeout(DATA_IDLE, s.read(&mut buf)) => match r {
                Ok(Ok(n)) => n,
                Ok(Err(e)) if ended(&e) => 0,
                Ok(Err(e)) => {
                    c.broken = true;
                    return Err(FtpError::Io(format!("{remote}: {e}")));
                }
                Err(_) => {
                    c.broken = true;
                    return Err(FtpError::Io(format!("{remote}: the server stopped sending")));
                }
            }
        };
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file.writer, &buf[..n]).map_err(|e| FtpError::Other(format!("Write error: {e}")))?;
        done += n as u64;
        if done - shown >= CHUNK as u64 {
            shown = done;
            progress(done, size.unwrap_or(done).max(done));
        }
    }
    drop(s);
    c.finish().await?;
    file.commit(cancel).map_err(|e| FtpError::Other(e.to_string()))?;
    progress(done, done);
    tracing::info!("FTP download complete: {remote} ({done} bytes)");
    Ok(done)
}

pub async fn download(
    conn: &FtpConn,
    remote: &str,
    local: &str,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> Result<u64, FtpError> {
    with_client!(conn, |c| download_on(c, remote, local, cancel, progress))
}

/// Send one local file; `progress` hears the bytes sent so far.
async fn send_file(
    c: &mut Client,
    local: &Path,
    remote: &str,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64) + Send,
) -> Result<u64, FtpError> {
    let mut file = tokio::fs::File::open(local)
        .await
        .map_err(|e| FtpError::Other(format!("Failed to read local file {}: {e}", local.display())))?;
    let mut s = c.data(&format!("STOR {remote}")).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut sent: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            drop(s);
            c.broken = true;
            return Err(FtpError::Cancelled);
        }
        let n = file.read(&mut buf).await.map_err(|e| FtpError::Other(format!("Failed to read local file {}: {e}", local.display())))?;
        if n == 0 {
            break;
        }
        let wrote = tokio::select! {
            _ = cancel.cancelled() => None,
            r = tokio::time::timeout(DATA_IDLE, s.write_all(&buf[..n])) => Some(r),
        };
        match wrote {
            None => {
                drop(s);
                c.broken = true;
                return Err(FtpError::Cancelled);
            }
            Some(Ok(Ok(()))) => {}
            Some(Ok(Err(e))) => {
                c.broken = true;
                return Err(FtpError::Io(format!("{remote}: {e}")));
            }
            Some(Err(_)) => {
                c.broken = true;
                return Err(FtpError::Io(format!("{remote}: the server stopped taking the data")));
            }
        }
        sent += n as u64;
        progress(sent);
    }
    if let Err(e) = s.shutdown().await {
        c.broken = true;
        return Err(FtpError::Io(format!("{remote}: {e}")));
    }
    drain(&mut s).await;
    drop(s);
    c.finish().await?;
    Ok(sent)
}

async fn upload_on(
    c: &mut Client,
    local: &str,
    remote: &str,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> Result<(), FtpError> {
    let path = Path::new(local);
    if path.is_dir() {
        let plan = plan_directory(path, remote).map_err(|e| FtpError::Other(format!("Failed to read local directory: {e}")))?;
        let total = plan.total_bytes;
        progress(0, total);
        for dir in &plan.dirs {
            if cancel.is_cancelled() {
                return Err(FtpError::Cancelled);
            }
            mkdirs(c, dir).await?;
        }
        let mut done: u64 = 0;
        for f in &plan.files {
            let mut shown = 0;
            let base = done;
            done += send_file(c, &f.local, &f.remote, cancel, |sent| {
                if sent - shown >= CHUNK as u64 {
                    shown = sent;
                    progress(base + sent, total);
                }
            })
            .await?;
        }
        progress(total, total);
    } else {
        let total = std::fs::metadata(path).map(|m| m.len()).map_err(|e| FtpError::Other(format!("Failed to read local file {local}: {e}")))?;
        progress(0, total);
        let mut shown = 0;
        let sent = send_file(c, path, remote, cancel, |sent| {
            if sent - shown >= CHUNK as u64 {
                shown = sent;
                progress(sent, total);
            }
        })
        .await?;
        progress(sent, sent);
    }
    tracing::info!("FTP upload complete: {remote}");
    Ok(())
}

/// Upload a file, or a folder and all in it. A cancelled file is removed
/// from the server, as the SFTP upload does.
pub async fn upload(
    conn: &FtpConn,
    local: &str,
    remote: &str,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> Result<(), FtpError> {
    let result = with_client!(conn, |c| upload_on(c, local, remote, cancel, progress));
    if matches!(result, Err(FtpError::Cancelled)) && !Path::new(local).is_dir() {
        let _ = with_client!(conn, |c| async { c.cmd(&format!("DELE {remote}")).await.map(|_| ()) });
    }
    result
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
