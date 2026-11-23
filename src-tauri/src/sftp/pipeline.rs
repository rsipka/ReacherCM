//! Reading and writing whole files over SFTP with many requests in flight.
//!
//! One request at a time waits a round trip per chunk; on a link with 50 ms
//! of latency that caps a 32 KiB chunk at 640 KiB/s. Keeping enough requests
//! in flight to fill the channel's window takes the latency out.
//!
//! Based on `read_remote_pipelined` / `write_remote_pipelined` from ReachQ
//! (github.com/CPTProgrammer/ReachQ, MIT), commit f3d9424, with these
//! changes: chunks come back in file order, so a download writes straight
//! through and a drag can stream; a read that returns less than was asked
//! (which the protocol allows, and some servers do) fetches the rest instead
//! of leaving a hole of zeros in the file; and downloads can be cancelled.

use std::collections::VecDeque;

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::backend::SftpProtocol;
use super::transfer::TransferError;

/// What an SFTP failure says, in the words the server used where it gave
/// any: "/root/x: Permission denied".
pub(crate) fn describe(path: &str, error: &SftpError) -> String {
    match error {
        SftpError::Status(status) => {
            let said = status.error_message.trim();
            if said.is_empty() {
                format!("{path}: {}", status.status_code)
            } else {
                format!("{path}: {said}")
            }
        }
        SftpError::Timeout => "The server did not answer in time".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn is_status(error: &SftpError, code: StatusCode) -> bool {
    matches!(error, SftpError::Status(s) if s.status_code == code)
}

fn failure(path: &str, error: SftpError) -> TransferError {
    TransferError::Remote(describe(path, &error))
}

fn task_failure(e: tokio::task::JoinError) -> TransferError {
    TransferError::IoError(format!("Transfer task ended: {e}"))
}

/// Read the remote file at `path`, `size` bytes of it, handing the data to
/// `sink` in file order. Returns how many bytes there were, which is fewer
/// than `size` if the file shrank while it was being read.
pub(crate) async fn read_file(
    proto: &SftpProtocol,
    path: &str,
    size: u64,
    cancel: &CancellationToken,
    mut sink: impl FnMut(&[u8]) -> Result<(), TransferError>,
    mut progress: impl FnMut(u64),
) -> Result<u64, TransferError> {
    let handle = proto
        .session
        .open(path, OpenFlags::READ, FileAttributes::empty())
        .await
        .map_err(|e| failure(path, e))?
        .handle;
    let result = read_open(proto, path, &handle, size, cancel, &mut sink, &mut progress).await;
    let _ = proto.session.close(handle).await;
    result
}

type ReadTask = JoinHandle<Result<Vec<u8>, SftpError>>;

fn spawn_read(proto: &SftpProtocol, handle: &str, offset: u64, len: u32) -> ReadTask {
    let session = proto.session.clone();
    let handle = handle.to_string();
    tokio::spawn(async move {
        match session.read(handle, offset, len).await {
            Ok(data) => Ok(data.data),
            // Past the end: the file is shorter than it was.
            Err(e) if is_status(&e, StatusCode::Eof) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    })
}

async fn read_open(
    proto: &SftpProtocol,
    path: &str,
    handle: &str,
    size: u64,
    cancel: &CancellationToken,
    sink: &mut impl FnMut(&[u8]) -> Result<(), TransferError>,
    progress: &mut impl FnMut(u64),
) -> Result<u64, TransferError> {
    let chunk = u64::from(proto.read_len);
    let depth = SftpProtocol::depth(proto.read_len);
    let mut queue: VecDeque<(u64, u32, ReadTask)> = VecDeque::new();
    let mut next = 0u64;
    let mut done = 0u64;

    let outcome = 'reading: loop {
        while next < size && queue.len() < depth {
            let len = chunk.min(size - next) as u32;
            queue.push_back((next, len, spawn_read(proto, handle, next, len)));
            next += u64::from(len);
        }
        let Some((offset, len, mut task)) = queue.pop_front() else {
            break Ok(done);
        };
        let data = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                task.abort();
                break Err(TransferError::Cancelled);
            }
            r = &mut task => r,
        };
        let data = match data {
            Ok(Ok(data)) => data,
            Ok(Err(e)) => break Err(failure(path, e)),
            Err(e) => break Err(task_failure(e)),
        };

        // A read may return less than was asked; the rest of this request's
        // range is fetched before going on, so nothing is skipped.
        let mut got = 0u64;
        let mut piece = data;
        loop {
            if piece.is_empty() {
                // The file ended early.
                break 'reading Ok(done);
            }
            if got + piece.len() as u64 > u64::from(len) {
                break 'reading Err(TransferError::IoError(
                    "The server sent more than was asked for".to_string(),
                ));
            }
            if let Err(e) = sink(&piece) {
                break 'reading Err(e);
            }
            got += piece.len() as u64;
            done += piece.len() as u64;
            if got == u64::from(len) {
                break;
            }
            let rest = (u64::from(len) - got) as u32;
            piece = match spawn_read(proto, handle, offset + got, rest).await {
                Ok(Ok(data)) => data,
                Ok(Err(e)) => break 'reading Err(failure(path, e)),
                Err(e) => break 'reading Err(task_failure(e)),
            };
        }
        progress(done);
    };

    for (_, _, task) in queue {
        task.abort();
    }
    outcome
}

/// Write a remote file at `path` from `next_chunk`, which hands out the
/// data in order, at most the given number of bytes at a time, and `None`
/// at the end. The file is created, or emptied if it exists. Returns how
/// many bytes were written.
pub(crate) async fn write_file(
    proto: &SftpProtocol,
    path: &str,
    mut next_chunk: impl FnMut(usize) -> Result<Option<Vec<u8>>, TransferError>,
    mut progress: impl FnMut(u64),
) -> Result<u64, TransferError> {
    let handle = proto
        .session
        .open(
            path,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            FileAttributes::empty(),
        )
        .await
        .map_err(|e| failure(path, e))?
        .handle;
    let result = write_open(proto, path, &handle, &mut next_chunk, &mut progress).await;
    // Some servers only report a failed write (a full disk) when the file
    // is closed, so a close that fails fails the write.
    let closed = proto.session.close(handle).await;
    match (result, closed) {
        (Ok(done), Ok(_)) => Ok(done),
        (Ok(_), Err(e)) => Err(failure(path, e)),
        (Err(e), _) => Err(e),
    }
}

async fn write_open(
    proto: &SftpProtocol,
    path: &str,
    handle: &str,
    next_chunk: &mut impl FnMut(usize) -> Result<Option<Vec<u8>>, TransferError>,
    progress: &mut impl FnMut(u64),
) -> Result<u64, TransferError> {
    let max_len = proto.write_len as usize;
    let depth = SftpProtocol::depth(proto.write_len);
    let mut queue: VecDeque<(u64, JoinHandle<Result<(), SftpError>>)> = VecDeque::new();
    let mut offset = 0u64;
    let mut done = 0u64;
    let mut at_end = false;

    let outcome = loop {
        while !at_end && queue.len() < depth {
            match next_chunk(max_len) {
                Ok(Some(data)) => {
                    let len = data.len() as u64;
                    let session = proto.session.clone();
                    let handle = handle.to_string();
                    let at = offset;
                    queue.push_back((
                        len,
                        tokio::spawn(async move { session.write(handle, at, data).await.map(|_| ()) }),
                    ));
                    offset += len;
                }
                Ok(None) => at_end = true,
                Err(e) => {
                    // What is in flight is abandoned below.
                    for (_, task) in queue.drain(..) {
                        task.abort();
                    }
                    return Err(e);
                }
            }
        }
        let Some((len, task)) = queue.pop_front() else {
            break Ok(done);
        };
        match task.await {
            Ok(Ok(())) => {
                done += len;
                progress(done);
            }
            Ok(Err(e)) => break Err(failure(path, e)),
            Err(e) => break Err(task_failure(e)),
        }
    };

    for (_, task) in queue {
        task.abort();
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::client::rawsession::RawSftpSession;
    use russh_sftp::protocol::{Data, Handle, Status};
    use std::sync::{Arc, Mutex};

    /// An SFTP server holding one file in memory, that answers a read with
    /// at most `max_read` bytes, as the protocol allows.
    struct OneFile {
        data: Arc<Mutex<Vec<u8>>>,
        max_read: usize,
    }

    fn ok(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        }
    }

    impl russh_sftp::server::Handler for OneFile {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn open(&mut self, id: u32, _: String, flags: OpenFlags, _: FileAttributes) -> Result<Handle, Self::Error> {
            if flags.contains(OpenFlags::TRUNCATE) {
                self.data.lock().unwrap().clear();
            }
            Ok(Handle { id, handle: "h".to_string() })
        }

        async fn close(&mut self, id: u32, _: String) -> Result<Status, Self::Error> {
            Ok(ok(id))
        }

        async fn read(&mut self, id: u32, _: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
            let data = self.data.lock().unwrap();
            let start = offset as usize;
            if start >= data.len() {
                return Err(StatusCode::Eof);
            }
            let end = data.len().min(start + (len as usize).min(self.max_read));
            Ok(Data { id, data: data[start..end].to_vec() })
        }

        async fn write(&mut self, id: u32, _: String, offset: u64, chunk: Vec<u8>) -> Result<Status, Self::Error> {
            let mut data = self.data.lock().unwrap();
            let end = offset as usize + chunk.len();
            if data.len() < end {
                data.resize(end, 0);
            }
            data[offset as usize..end].copy_from_slice(&chunk);
            Ok(ok(id))
        }
    }

    async fn serve(content: Vec<u8>, max_read: usize) -> (SftpProtocol, Arc<Mutex<Vec<u8>>>) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let data = Arc::new(Mutex::new(content));
        russh_sftp::server::run(server, OneFile { data: data.clone(), max_read }).await;
        let session = RawSftpSession::new(client);
        session.init().await.unwrap();
        (SftpProtocol { session: Arc::new(session), read_len: 32 * 1024, write_len: 32 * 1024 }, data)
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    async fn read_all(proto: &SftpProtocol, size: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let n = read_file(proto, "/f", size, &CancellationToken::new(), |d| { out.extend_from_slice(d); Ok(()) }, |_| {})
            .await
            .unwrap();
        assert_eq!(n as usize, out.len());
        out
    }

    #[tokio::test]
    async fn replacement_download_preserves_original_until_success() {
        use crate::sftp::transfer::download_sftp_contents;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report");
        std::fs::write(&path, b"original").unwrap();
        let data = pattern(800_000);
        let (proto, _) = serve(data.clone(), usize::MAX).await;
        let cancel = CancellationToken::new();
        let result = download_sftp_contents(&proto, "/f", &crate::sftp::local_download::DownloadTarget::Chosen(path.clone()), Some(data.len() as u64), &cancel, |_| {
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
            cancel.cancel();
        }).await;
        assert!(matches!(result, Err(TransferError::Cancelled)));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        download_sftp_contents(&proto, "/f", &crate::sftp::local_download::DownloadTarget::Chosen(path.clone()), None, &CancellationToken::new(), |_| {}).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn a_file_reads_whole_and_in_order() {
        let content = pattern(1_000_003);
        let (proto, _) = serve(content.clone(), usize::MAX).await;
        assert_eq!(read_all(&proto, content.len() as u64).await, content);
    }

    /// ReachQ's version moved on to the next offset after a short read and
    /// left the rest of the chunk as zeros in the file.
    #[tokio::test]
    async fn a_short_read_leaves_no_hole() {
        let content = pattern(300_000);
        let (proto, _) = serve(content.clone(), 10_000).await;
        assert_eq!(read_all(&proto, content.len() as u64).await, content);
    }

    #[tokio::test]
    async fn a_file_that_shrank_reads_as_it_is_now() {
        let content = pattern(50_000);
        let (proto, _) = serve(content.clone(), usize::MAX).await;
        assert_eq!(read_all(&proto, 200_000).await, content);
    }

    #[tokio::test]
    async fn a_file_writes_whole() {
        let content = pattern(777_777);
        let (proto, stored) = serve(b"old content that must go".to_vec(), usize::MAX).await;
        let mut rest = &content[..];
        let next = |max: usize| {
            let (now, later) = rest.split_at(max.min(rest.len()));
            rest = later;
            Ok((!now.is_empty()).then(|| now.to_vec()))
        };
        let mut reported = 0;
        let n = write_file(&proto, "/f", next, |d| reported = d).await.unwrap();
        assert_eq!(n, content.len() as u64);
        assert_eq!(reported, n);
        assert_eq!(*stored.lock().unwrap(), content);
    }

    #[tokio::test]
    async fn a_cancelled_transfer_stops() {
        let (proto, _) = serve(pattern(1_000_000), usize::MAX).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let r = read_file(&proto, "/f", 1_000_000, &cancel, |_| Ok(()), |_| {}).await;
        assert!(matches!(r, Err(TransferError::Cancelled)));
    }
}
