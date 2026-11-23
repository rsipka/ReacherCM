//! Copies over the scp protocol (the old `rcp` one: `scp -t` takes a file,
//! `scp -f` sends one), for SCP sessions: servers with no SFTP subsystem,
//! where the base64 shell path would have to hold the file twice over.
//! Folders are browsed and changed with shell commands as without SFTP.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::local_download::LocalDownload;
use super::transfer::{shell_path, CancellationToken, TransferError};
use crate::ssh::client::SharedHandle;

const CHUNK: usize = 256 * 1024;
/// How long the far side may stay silent.
const IDLE: Duration = Duration::from_secs(60);

fn broken(e: impl std::fmt::Display) -> TransferError {
    TransferError::Remote(format!("scp: {e}"))
}

async fn exec(handle: &SharedHandle, command: &str) -> Result<impl AsyncRead + AsyncWrite + Unpin + Send, TransferError> {
    let channel = {
        let guard = handle.lock().await;
        guard.channel_open_session().await.map_err(broken)?
    };
    channel.exec(true, command).await.map_err(broken)?;
    Ok(channel.into_stream())
}

async fn read_byte<S: AsyncRead + Unpin>(s: &mut S) -> Result<u8, TransferError> {
    let mut b = [0u8; 1];
    match tokio::time::timeout(IDLE, s.read_exact(&mut b)).await {
        Ok(Ok(_)) => Ok(b[0]),
        Ok(Err(_)) => Err(TransferError::Remote("scp: the server closed the copy (is scp installed there?)".into())),
        Err(_) => Err(TransferError::Remote("scp: the server stopped answering".into())),
    }
}

async fn read_line<S: AsyncRead + Unpin>(s: &mut S) -> Result<String, TransferError> {
    let mut line = Vec::new();
    loop {
        let b = read_byte(s).await?;
        if b == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        if line.len() > 4096 {
            return Err(TransferError::Remote("scp: a reply line too long".into()));
        }
        line.push(b);
    }
}

/// The far side's answer: 0 is yes, 1 and 2 come with what went wrong.
async fn ack<S: AsyncRead + Unpin>(s: &mut S) -> Result<(), TransferError> {
    match read_byte(s).await? {
        0 => Ok(()),
        1 | 2 => Err(TransferError::Remote(read_line(s).await?.trim().to_string())),
        other => Err(TransferError::Remote(format!("scp: unexpected reply {other:#04x}"))),
    }
}

/// A file's header from the source: `C0644 1234 name`.
pub fn parse_header(line: &str) -> Option<(u32, u64, &str)> {
    let rest = line.strip_prefix('C')?;
    let mut parts = rest.splitn(3, ' ');
    let mode = u32::from_str_radix(parts.next()?, 8).ok()?;
    let size = parts.next()?.parse().ok()?;
    let name = parts.next()?;
    Some((mode, size, name))
}

fn header(mode: u32, size: u64, name: &str) -> Result<String, TransferError> {
    if name.contains(['\n', '\r', '/']) || name.is_empty() {
        return Err(TransferError::Remote(format!("scp: '{name}' cannot be sent by name")));
    }
    Ok(format!("C{:04o} {size} {name}\n", mode & 0o7777))
}

#[cfg(unix)]
fn local_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn local_mode(_meta: &std::fs::Metadata) -> u32 {
    0o644
}

/// Send one local file to `remote_path` (a file's path, not a folder).
pub async fn send_file(
    handle: &SharedHandle,
    local: &Path,
    remote_path: &str,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64),
) -> Result<u64, TransferError> {
    let meta = std::fs::metadata(local).map_err(|e| TransferError::IoError(format!("Failed to read local file {}: {e}", local.display())))?;
    let size = meta.len();
    let name = remote_path.rsplit('/').next().unwrap_or(remote_path);
    let line = header(local_mode(&meta), size, name)?;
    let mut file = tokio::fs::File::open(local)
        .await
        .map_err(|e| TransferError::IoError(format!("Failed to read local file {}: {e}", local.display())))?;

    // `-t` to the path itself: the name in the header is then ignored for
    // a file that is not a folder, and the file lands where asked.
    let mut s = exec(handle, &format!("scp -t {}", shell_path(remote_path))).await?;
    ack(&mut s).await?;
    s.write_all(line.as_bytes()).await.map_err(broken)?;
    ack(&mut s).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    while sent < size {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let want = (size - sent).min(CHUNK as u64) as usize;
        let n = file.read(&mut buf[..want]).await.map_err(|e| TransferError::IoError(format!("Failed to read local file {}: {e}", local.display())))?;
        if n == 0 {
            return Err(TransferError::IoError(format!("{}: the file got shorter while it was sent", local.display())));
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(TransferError::Cancelled),
            r = tokio::time::timeout(IDLE, s.write_all(&buf[..n])) => match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(broken(e)),
                Err(_) => return Err(TransferError::Remote("scp: the server stopped taking the data".into())),
            }
        }
        sent += n as u64;
        on_progress(sent);
    }
    s.write_all(&[0]).await.map_err(broken)?;
    ack(&mut s).await?;
    let _ = s.shutdown().await;
    Ok(sent)
}

/// Fetch `remote_path` into `local_path` through a staging file.
pub async fn receive_file(
    handle: &SharedHandle,
    remote_path: &str,
    local_path: &str,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<u64, TransferError> {
    let mut s = exec(handle, &format!("scp -f {}", shell_path(remote_path))).await?;
    s.write_all(&[0]).await.map_err(broken)?;
    let line = loop {
        match read_byte(&mut s).await? {
            1 | 2 => return Err(TransferError::Remote(read_line(&mut s).await?.trim().to_string())),
            b'C' => break format!("C{}", read_line(&mut s).await?),
            // Times (-p), which were not asked for: acknowledged and passed over.
            b'T' => {
                read_line(&mut s).await?;
                s.write_all(&[0]).await.map_err(broken)?;
            }
            b'D' => return Err(TransferError::Remote(format!("{remote_path}: is a folder"))),
            other => return Err(TransferError::Remote(format!("scp: unexpected reply {other:#04x}"))),
        }
    };
    let (_, size, _) = parse_header(&line).ok_or_else(|| TransferError::Remote(format!("scp: cannot read \"{line}\"")))?;
    let mut file = LocalDownload::new(Path::new(local_path))?;
    s.write_all(&[0]).await.map_err(broken)?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    on_progress(0, size);
    while done < size {
        let want = (size - done).min(CHUNK as u64) as usize;
        let n = tokio::select! {
            _ = cancel.cancelled() => return Err(TransferError::Cancelled),
            r = tokio::time::timeout(IDLE, s.read(&mut buf[..want])) => match r {
                Ok(Ok(0)) => return Err(TransferError::Remote("scp: the server closed the copy early".into())),
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(broken(e)),
                Err(_) => return Err(TransferError::Remote("scp: the server stopped sending".into())),
            }
        };
        std::io::Write::write_all(&mut file.writer, &buf[..n]).map_err(|e| TransferError::IoError(format!("Write error: {e}")))?;
        done += n as u64;
        on_progress(done, size);
    }
    ack(&mut s).await?;
    s.write_all(&[0]).await.map_err(broken)?;
    let _ = s.shutdown().await;
    file.commit(cancel)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        assert_eq!(parse_header("C0644 1234 a b.txt"), Some((0o644, 1234, "a b.txt")));
        assert_eq!(parse_header("D0755 0 dir"), None);
        assert_eq!(header(0o100644, 5, "x").unwrap(), "C0644 5 x\n");
        assert!(header(0o644, 5, "a\nb").is_err());
    }

    #[tokio::test]
    async fn an_error_reply_says_why() {
        let mut input: &[u8] = b"\x01scp: /x: Permission denied\n";
        let e = ack(&mut input).await.unwrap_err();
        assert_eq!(e.to_string(), "scp: /x: Permission denied");
    }
}
