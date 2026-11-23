use serde::{Deserialize, Serialize};
use thiserror::Error;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio_util::sync::CancellationToken;
use crate::ssh::client::{SharedHandle, exec_on_connection, exec_on_connection_with_exit_code, SshError};
use super::backend::{RemoteFs, SftpProtocol};
use super::pipeline;

#[derive(Debug, Error)]
pub enum SftpBrowserError {
    #[error("Not connected")]
    NotConnected,
    #[error("SSH error: {0}")]
    SshError(#[from] SshError),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Path not found: {0}")]
    PathNotFound(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// A remote command failed; what it said, e.g. "base64: /x: Permission denied".
    #[error("{0}")]
    Remote(String),
}

/// Metadata for a remote file or directory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "isDirectory")]
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub permissions: String,
    /// The owning user and group, as `ls -l` names them (a number when
    /// the server has no name for the id).
    pub owner: String,
    pub group: String,
}

// --- SFTP ------------------------------------------------------------------
//
// Each operation below uses the SFTP protocol when the connection has it
// (see `backend`) and the shell commands the explorer has always used when
// it does not. Once SFTP is there nothing falls back to the shell: an
// SFTP-only account (ForceCommand internal-sftp) runs every command as the
// SFTP server, which never answers it.

/// An SFTP failure as an explorer error. A failure of the session itself
/// drops the session, so the next operation opens a new one.
async fn sftp_failed(fs: &RemoteFs, path: &str, e: SftpError) -> SftpBrowserError {
    fs.note_error(&e).await;
    match &e {
        SftpError::Status(s) if s.status_code == StatusCode::NoSuchFile => {
            SftpBrowserError::PathNotFound(path.to_string())
        }
        SftpError::Status(s) if s.status_code == StatusCode::PermissionDenied => {
            SftpBrowserError::PermissionDenied(path.to_string())
        }
        _ => SftpBrowserError::Remote(pipeline::describe(path, &e)),
    }
}

/// `ls -l` style permissions (`drwxr-sr-x`) from a mode, so a listing looks
/// the same whichever way it was made.
pub(crate) fn format_permissions(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o140000 => 's',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        _ => '-',
    };
    let mut s = String::with_capacity(10);
    s.push(kind);
    let triplets = [(0o4000, 6, 's', 'S'), (0o2000, 3, 's', 'S'), (0o1000, 0, 't', 'T')];
    for (special, shift, set_exec, set_no_exec) in triplets {
        let bits = (mode >> shift) & 0o7;
        s.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        s.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        s.push(match (mode & special != 0, bits & 0o1 != 0) {
            (true, true) => set_exec,
            (true, false) => set_no_exec,
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

/// Owner and group as the server's own `ls -l` line names them. SFTP v3
/// sends ids only, plus that line, which most servers fill in.
fn owner_group(longname: &str, attrs: &FileAttributes) -> (String, String) {
    let parts: Vec<&str> = longname.split_whitespace().collect();
    let looks_like_ls = parts.len() >= 4
        && parts[0].len() >= 10
        && parts[0].starts_with(|c: char| "d-lcbps".contains(c));
    if looks_like_ls {
        return (parts[2].to_string(), parts[3].to_string());
    }
    let name = |n: &Option<String>, id: Option<u32>| {
        n.clone().or(id.map(|i| i.to_string())).unwrap_or_default()
    };
    (name(&attrs.user, attrs.uid), name(&attrs.group, attrs.gid))
}

fn sort_entries(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// Every entry in `path`, unsorted, `.` and `..` left out.
async fn read_dir_sftp(
    fs: &RemoteFs,
    proto: &SftpProtocol,
    path: &str,
) -> Result<Vec<RemoteEntry>, SftpBrowserError> {
    let handle = match proto.session.opendir(path).await {
        Ok(h) => h.handle,
        Err(e) => return Err(sftp_failed(fs, path, e).await),
    };
    let mut entries = Vec::new();
    let result = loop {
        match proto.session.readdir(handle.clone()).await {
            Ok(name) => {
                for f in name.files {
                    if f.filename == "." || f.filename == ".." {
                        continue;
                    }
                    let mode = f.attrs.permissions.unwrap_or(0);
                    let (owner, group) = owner_group(&f.longname, &f.attrs);
                    entries.push(RemoteEntry {
                        path: join(path, &f.filename),
                        name: f.filename,
                        is_dir: mode & 0o170000 == 0o040000,
                        size: f.attrs.size.unwrap_or(0),
                        modified: f.attrs.mtime.map(u64::from).unwrap_or(0),
                        permissions: format_permissions(mode),
                        owner,
                        group,
                    });
                }
            }
            Err(e) if pipeline::is_status(&e, StatusCode::Eof) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    let _ = proto.session.close(handle).await;
    match result {
        Ok(()) => Ok(entries),
        Err(e) => Err(sftp_failed(fs, path, e).await),
    }
}

/// List the contents of a remote directory.
pub async fn list_directory(
    fs: &RemoteFs,
    path: &str,
) -> Result<Vec<RemoteEntry>, SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        let mut entries = read_dir_sftp(fs, &proto, path).await?;
        sort_entries(&mut entries);
        return Ok(entries);
    }
    // Use ls -lA --time-style=+%s to get machine-parseable output with timestamps
    let command = format!(
        "ls -lA --time-style=+%s {} 2>/dev/null || ls -lA {}",
        shell_escape(path),
        shell_escape(path)
    );
    let (stdout, stderr, code) = exec_on_connection_with_exit_code(&fs.handle, &command).await?;
    parse_ls_output(&listing_output(stdout, &stderr, code)?, path)
}

/// The login folder: SFTP's `.`, else what `pwd` says. `None` when
/// neither answers with a path.
pub async fn home_dir(fs: &RemoteFs) -> Option<String> {
    if let Some(proto) = fs.sftp().await {
        if let Ok(name) = proto.session.realpath(".").await {
            return name.files.first().map(|f| f.filename.clone()).filter(|p| p.starts_with('/'));
        }
    }
    let out = exec_on_connection(&fs.handle, "pwd").await.ok()?;
    let path = out.trim();
    path.starts_with('/').then(|| path.to_string())
}

/// Create a directory on the remote host, and any parents it needs.
pub async fn make_directory(
    fs: &RemoteFs,
    path: &str,
) -> Result<(), SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        return make_directory_sftp(fs, &proto, path).await;
    }
    let command = format!("mkdir -p {}", shell_escape(path));
    run_checked(&fs.handle, &command).await?;
    Ok(())
}

/// `mkdir -p` over SFTP, whose own mkdir makes one level and fails on one
/// that is there.
pub(crate) async fn make_directory_sftp(
    fs: &RemoteFs,
    proto: &SftpProtocol,
    path: &str,
) -> Result<(), SftpBrowserError> {
    // The missing part of the path, deepest first.
    let mut missing = Vec::new();
    let mut at = path.trim_end_matches('/').to_string();
    while !at.is_empty() {
        match proto.session.stat(at.clone()).await {
            Ok(attrs) if attrs.attrs.is_dir() => break,
            Ok(_) => {
                return Err(SftpBrowserError::Remote(format!("{at}: exists and is not a folder")));
            }
            Err(e) if pipeline::is_status(&e, StatusCode::NoSuchFile) => {
                missing.push(at.clone());
                match at.rfind('/') {
                    Some(i) => at.truncate(i),
                    None => break,
                }
            }
            Err(e) => return Err(sftp_failed(fs, &at, e).await),
        }
    }
    for dir in missing.into_iter().rev() {
        if let Err(e) = proto.session.mkdir(dir.clone(), FileAttributes::empty()).await {
            return Err(sftp_failed(fs, &dir, e).await);
        }
    }
    Ok(())
}

/// Delete a file or directory on the remote host.
pub async fn delete_entry(
    fs: &RemoteFs,
    path: &str,
) -> Result<(), SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        return delete_entry_sftp(fs, &proto, path).await;
    }
    let command = format!("rm -rf {}", shell_escape(path));
    run_checked(&fs.handle, &command).await?;
    Ok(())
}

/// How many removals a recursive delete keeps in flight.
const DELETE_DEPTH: usize = 16;

/// `rm -rf` over SFTP: list the tree, remove its files many at a time, then
/// its folders deepest first. A link is removed, never followed. ReachQ left
/// this to the shell, which an SFTP-only account does not have.
async fn delete_entry_sftp(
    fs: &RemoteFs,
    proto: &SftpProtocol,
    path: &str,
) -> Result<(), SftpBrowserError> {
    let top = match proto.session.lstat(path).await {
        Ok(a) => a.attrs,
        Err(e) => return Err(sftp_failed(fs, path, e).await),
    };
    if !top.is_dir() {
        return match proto.session.remove(path).await {
            Ok(_) => Ok(()),
            Err(e) => Err(sftp_failed(fs, path, e).await),
        };
    }

    // Folders in the order found, which puts every parent before its children.
    let mut dirs = vec![path.to_string()];
    let mut files = Vec::new();
    let mut i = 0;
    while i < dirs.len() {
        let dir = dirs[i].clone();
        for entry in read_dir_sftp(fs, proto, &dir).await? {
            if entry.is_dir {
                dirs.push(entry.path);
            } else {
                files.push(entry.path);
            }
        }
        i += 1;
    }

    let mut running = tokio::task::JoinSet::new();
    let mut first_error: Option<(String, SftpError)> = None;
    for file in files {
        while running.len() >= DELETE_DEPTH {
            if let Some(Ok(Err(failed))) = running.join_next().await {
                first_error.get_or_insert(failed);
            }
        }
        if first_error.is_some() {
            break;
        }
        let session = proto.session.clone();
        running.spawn(async move {
            session.remove(file.clone()).await.map(|_| ()).map_err(|e| (file, e))
        });
    }
    while let Some(done) = running.join_next().await {
        if let Ok(Err(failed)) = done {
            first_error.get_or_insert(failed);
        }
    }
    if let Some((file, e)) = first_error {
        return Err(sftp_failed(fs, &file, e).await);
    }

    for dir in dirs.iter().rev() {
        if let Err(e) = proto.session.rmdir(dir.clone()).await {
            return Err(sftp_failed(fs, dir, e).await);
        }
    }
    Ok(())
}

/// Create an empty file on the remote host. An existing file keeps what is
/// in it.
pub async fn touch_file(
    fs: &RemoteFs,
    path: &str,
) -> Result<(), SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        let opened = proto
            .session
            .open(path, OpenFlags::CREATE | OpenFlags::WRITE, FileAttributes::empty())
            .await;
        return match opened {
            Ok(h) => match proto.session.close(h.handle).await {
                Ok(_) => Ok(()),
                Err(e) => Err(sftp_failed(fs, path, e).await),
            },
            Err(e) => Err(sftp_failed(fs, path, e).await),
        };
    }
    let command = format!("touch {}", shell_escape(path));
    run_checked(&fs.handle, &command).await?;
    Ok(())
}

fn already_exists(path: &str) -> SftpBrowserError {
    SftpBrowserError::Remote(format!("{path}: a file or folder of that name already exists"))
}

/// Rename or move a remote file or directory. Fails when something is
/// already at `new_path`: `mv` alone would replace a file, or move the
/// entry into a folder of that name.
pub async fn rename_entry(
    fs: &RemoteFs,
    old_path: &str,
    new_path: &str,
) -> Result<(), SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        match proto.session.lstat(new_path).await {
            Ok(_) => return Err(already_exists(new_path)),
            Err(e) if pipeline::is_status(&e, StatusCode::NoSuchFile) => {}
            Err(e) => return Err(sftp_failed(fs, new_path, e).await),
        }
        return match proto.session.rename(old_path, new_path).await {
            Ok(_) => Ok(()),
            Err(e) => Err(sftp_failed(fs, old_path, e).await),
        };
    }
    let new = shell_escape(new_path);
    let command = format!(
        "if [ -e {new} ] || [ -L {new} ]; then echo EXISTS; exit 1; fi; mv {} {new}",
        shell_escape(old_path)
    );
    let (stdout, stderr, code) = exec_on_connection_with_exit_code(&fs.handle, &command).await?;
    if code != 0 && stdout.trim() == "EXISTS" {
        return Err(already_exists(new_path));
    }
    command_output(stdout, &stderr, code)?;
    Ok(())
}

/// Maximum file size for text editing (5 MB).
const MAX_EDIT_SIZE: u64 = 5 * 1024 * 1024;

fn too_large(size: u64) -> SftpBrowserError {
    SftpBrowserError::ParseError(format!(
        "File too large to edit ({:.1} MB, max {:.0} MB)",
        size as f64 / (1024.0 * 1024.0),
        MAX_EDIT_SIZE as f64 / (1024.0 * 1024.0)
    ))
}

fn transfer_failed(e: super::transfer::TransferError) -> SftpBrowserError {
    SftpBrowserError::Remote(match e {
        super::transfer::TransferError::Remote(said) => said,
        other => other.to_string(),
    })
}

/// Read a text file's content from the remote host.
pub async fn read_text_file(
    fs: &RemoteFs,
    path: &str,
) -> Result<String, SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        let size = match proto.session.stat(path).await {
            Ok(a) => a.attrs.size.unwrap_or(0),
            Err(e) => return Err(sftp_failed(fs, path, e).await),
        };
        if size > MAX_EDIT_SIZE {
            return Err(too_large(size));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        let never = CancellationToken::new();
        pipeline::read_file(&proto, path, size, &never, |d| { bytes.extend_from_slice(d); Ok(()) }, |_| {})
            .await
            .map_err(transfer_failed)?;
        return String::from_utf8(bytes)
            .map_err(|_| SftpBrowserError::ParseError("File is not valid UTF-8 text".to_string()));
    }
    read_text_file_exec(&fs.handle, path).await
}

/// Read a text file's content from the remote host via base64 encoding.
async fn read_text_file_exec(
    handle: &SharedHandle,
    path: &str,
) -> Result<String, SftpBrowserError> {
    // Check file size first
    let stat_cmd = format!("stat -c %s {} 2>/dev/null || stat -f %z {}", shell_escape(path), shell_escape(path));
    let size_output = exec_on_connection(handle, &stat_cmd).await?;
    let size: u64 = size_output
        .trim()
        .parse()
        .map_err(|_| SftpBrowserError::ParseError(format!("Cannot determine file size: {}", path)))?;

    if size > MAX_EDIT_SIZE {
        return Err(too_large(size));
    }

    // Read file via base64 to handle binary-safe transport
    let cmd = format!("base64 {}", shell_escape(path));
    let (stdout, stderr, code) = exec_on_connection_with_exit_code(handle, &cmd).await?;
    let b64_output = command_output(stdout, &stderr, code)?;

    // Remove all whitespace from base64 output (line breaks etc.)
    let b64_clean: String = b64_output.chars().filter(|c| !c.is_whitespace()).collect();

    if b64_clean.is_empty() {
        return Ok(String::new());
    }

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&b64_clean)
        .map_err(|e| SftpBrowserError::ParseError(format!("Base64 decode failed: {}", e)))?;

    String::from_utf8(bytes).map_err(|_| {
        SftpBrowserError::ParseError("File is not valid UTF-8 text".to_string())
    })
}

/// What `ls` listed. It also fails for an entry it could not stat (exit 1,
/// the rest still listed), so only a failure that listed nothing is an
/// error: a folder the login user cannot read, which showed as empty (#150).
fn listing_output(stdout: String, stderr: &str, code: i32) -> Result<String, SftpBrowserError> {
    let listed = stdout.lines().any(|l| !l.trim().is_empty() && !l.starts_with("total "));
    if listed { Ok(stdout) } else { command_output(stdout, stderr, code) }
}

/// Run a remote command; its stdout, or an error with what it said.
///
/// The commands here (mkdir, rm, touch, mv) print nothing when they work,
/// and when they fail only stderr and the exit status say so: a delete the
/// login user may not do reported success, and the file came back on the
/// next refresh (#150).
async fn run_checked(handle: &SharedHandle, command: &str) -> Result<String, SftpBrowserError> {
    let (stdout, stderr, code) = exec_on_connection_with_exit_code(handle, command).await?;
    command_output(stdout, &stderr, code)
}

/// A remote command's stdout, if it succeeded. A file the login user cannot
/// read gives an empty stdout and a reason on stderr; taken as output, it
/// opened as an empty file, and saving that would have emptied the real one
/// (#150). `-1` is no exit status at all: the channel closed or timed out.
fn command_output(stdout: String, stderr: &str, code: i32) -> Result<String, SftpBrowserError> {
    if code == 0 {
        return Ok(stdout);
    }
    let said = stderr.trim();
    Err(SftpBrowserError::Remote(match (code, said.is_empty()) {
        (-1, _) => "The connection closed before the server answered".to_string(),
        (_, false) => said.to_string(),
        (_, true) => format!("The server reported a failure (exit status {code})"),
    }))
}

/// Write text content to a remote file.
pub async fn write_text_file(
    fs: &RemoteFs,
    path: &str,
    content: &str,
) -> Result<(), SftpBrowserError> {
    if let Some(proto) = fs.sftp().await {
        let mut rest = content.as_bytes();
        let never = CancellationToken::new();
        let next = |max: usize| {
            let (now, later) = rest.split_at(max.min(rest.len()));
            rest = later;
            Ok((!now.is_empty()).then(|| now.to_vec()))
        };
        pipeline::write_file(&proto, path, &never, next, |_| {})
            .await
            .map_err(transfer_failed)?;
        return Ok(());
    }
    write_text_file_exec(&fs.handle, path, content).await
}

/// Write text content to a remote file via streaming base64 over a single SSH channel.
async fn write_text_file_exec(
    handle: &SharedHandle,
    path: &str,
    content: &str,
) -> Result<(), SftpBrowserError> {
    use base64::Engine;
    use russh::ChannelMsg;

    let data = content.as_bytes();

    // Empty file: simple truncate
    if data.is_empty() {
        let cmd = format!(": > {}", shell_escape(path));
        run_checked(handle, &cmd).await?;
        return Ok(());
    }

    // Open a single channel: pipe base64 stdin into decoder, write to file
    let mut channel = {
        let guard = handle.lock().await;
        guard.channel_open_session().await
            .map_err(|e| SshError::ChannelError(format!("{}", e)))?
    };
    channel.exec(true, format!("base64 -d > {}", shell_escape(path))).await
        .map_err(|e| SshError::ChannelError(format!("{}", e)))?;

    // Stream base64 in 48KB raw chunks (multiple of 3 → clean base64, no mid-stream padding)
    let chunk_size: usize = 48 * 1024;

    for chunk in data.chunks(chunk_size) {
        let mut b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
        b64.push('\n');

        channel.data(b64.as_bytes()).await
            .map_err(|e| SftpBrowserError::ParseError(format!("Channel write error: {}", e)))?;
    }

    // Close stdin to signal EOF
    channel.eof().await
        .map_err(|e| SftpBrowserError::ParseError(format!("EOF signal error: {}", e)))?;

    // Wait for exit
    let mut got_eof = false;
    let mut got_exit = false;
    let mut exit_code: Option<u32> = None;
    let mut stderr_buf = String::new();

    loop {
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            channel.wait(),
        ).await;

        match msg {
            Ok(Some(ChannelMsg::ExtendedData { ref data, .. })) => {
                stderr_buf.push_str(&String::from_utf8_lossy(data));
            }
            Ok(Some(ChannelMsg::Eof)) => {
                got_eof = true;
                if got_exit { break; }
            }
            Ok(Some(ChannelMsg::ExitStatus { exit_status })) => {
                exit_code = Some(exit_status);
                got_exit = true;
                if got_eof { break; }
            }
            Ok(None) | Err(_) => break,
            _ => {}
        }
    }

    if let Some(code) = exit_code {
        if code != 0 {
            let msg = stderr_buf.trim();
            if msg.to_lowercase().contains("permission denied") {
                return Err(SftpBrowserError::PermissionDenied(path.to_string()));
            }
            return Err(SftpBrowserError::ParseError(
                if msg.is_empty() { format!("Write failed with exit code {}", code) } else { msg.to_string() }
            ));
        }
    }

    Ok(())
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn parse_ls_output(output: &str, base_path: &str) -> Result<Vec<RemoteEntry>, SftpBrowserError> {
    let mut entries = Vec::new();
    let base = if base_path.ends_with('/') {
        base_path.to_string()
    } else {
        format!("{}/", base_path)
    };

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }

        let permissions = parts[0].to_string();
        // Skip non-entry lines (permissions must start with d, -, l, c, b, p, s)
        if !permissions.starts_with(|c: char| "d-lcbps".contains(c)) {
            continue;
        }
        let is_dir = permissions.starts_with('d');
        let owner = parts[2].to_string();
        let group = parts[3].to_string();
        let size: u64 = parts[4].parse().unwrap_or(0);

        // Detect format: --time-style=+%s gives epoch in parts[5], standard ls gives month
        let (modified, name) = if parts[5].parse::<u64>().is_ok() {
            // Epoch format: perms links owner group size epoch name...
            // parts: [0]=perms [1]=links [2]=owner [3]=group [4]=size [5]=epoch [6..]=name
            let ts = parts[5].parse::<u64>().unwrap_or(0);
            let name = parts[6..].join(" ");
            (ts, name)
        } else {
            // Standard format: perms links owner group size month day time/year name...
            // parts: [0]=perms [1]=links [2]=owner [3]=group [4]=size [5]=month [6]=day [7]=time [8..]=name
            if parts.len() < 9 {
                // Might be a short format, take last field as name
                let name = parts[parts.len() - 1].to_string();
                (0u64, name)
            } else {
                let name = parts[8..].join(" ");
                (0u64, name)
            }
        };

        if name == "." || name == ".." || name.is_empty() {
            continue;
        }

        // Handle symlinks: name -> target
        let clean_name = if let Some(idx) = name.find(" -> ") {
            name[..idx].to_string()
        } else {
            name
        };

        entries.push(RemoteEntry {
            path: format!("{}{}", base, clean_name),
            name: clean_name,
            is_dir,
            size,
            modified,
            permissions,
            owner,
            group,
        });
    }

    // Sort: directories first, then by name
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

#[cfg(test)]
mod command_output_tests {
    use super::*;

    #[test]
    fn a_command_that_succeeded_gives_its_output() {
        assert_eq!(command_output("aGkK\n".into(), "", 0).unwrap(), "aGkK\n");
        // An empty file is a success with nothing on stdout.
        assert_eq!(command_output(String::new(), "", 0).unwrap(), "");
    }

    /// #150: a file the login user cannot read opened as an empty file.
    #[test]
    fn a_file_the_server_cannot_read_is_an_error_saying_why() {
        let err = command_output(String::new(), "base64: /root/x: Permission denied\n", 1).unwrap_err();
        assert_eq!(err.to_string(), "base64: /root/x: Permission denied");
    }

    #[test]
    fn a_failure_with_nothing_on_stderr_still_fails() {
        let err = command_output(String::new(), "", 1).unwrap_err();
        assert_eq!(err.to_string(), "The server reported a failure (exit status 1)");
    }

    #[test]
    fn a_folder_the_server_cannot_list_is_an_error_saying_why() {
        let err = listing_output(String::new(), "ls: cannot open directory '/root': Permission denied
", 2).unwrap_err();
        assert_eq!(err.to_string(), "ls: cannot open directory '/root': Permission denied");
    }

    #[test]
    fn a_folder_listed_in_part_still_lists() {
        let out = "total 4
-rw-r--r-- 1 a a 3 1700000000 ok.txt
".to_string();
        let listed = listing_output(out.clone(), "ls: cannot access 'gone': No such file or directory
", 1).unwrap();
        assert_eq!(listed, out);
    }

    #[test]
    fn an_empty_folder_is_not_an_error() {
        assert_eq!(listing_output("total 0
".into(), "", 0).unwrap(), "total 0
");
    }

    #[test]
    fn a_listing_keeps_owner_group_size_and_time() {
        let out = "total 8
drwxr-xr-x 2 root root 4096 1762000000 etc
-rw-r----- 1 www-data adm 1234 1762000001 my file.log
lrwxrwxrwx 1 root root 7 1762000002 bin -> usr/bin
";
        let e = parse_ls_output(out, "/").unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].name.as_str(), e[0].owner.as_str(), e[0].group.as_str()), ("etc", "root", "root"));
        let log = e.iter().find(|x| x.name == "my file.log").unwrap();
        assert_eq!((log.owner.as_str(), log.group.as_str(), log.size, log.modified), ("www-data", "adm", 1234, 1762000001));
        assert_eq!(log.permissions, "-rw-r-----");
        assert!(e.iter().any(|x| x.name == "bin" && x.path == "/bin"));
    }

    #[test]
    fn no_exit_status_is_a_file_cut_short() {
        let err = command_output("aGVsbG8g".into(), "", -1).unwrap_err();
        assert_eq!(err.to_string(), "The connection closed before the server answered");
    }
}

#[cfg(test)]
mod sftp_listing_tests {
    use super::*;

    #[test]
    fn permissions_read_as_ls_shows_them() {
        assert_eq!(format_permissions(0o040755), "drwxr-xr-x");
        assert_eq!(format_permissions(0o100644), "-rw-r--r--");
        assert_eq!(format_permissions(0o120777), "lrwxrwxrwx");
        assert_eq!(format_permissions(0o104755), "-rwsr-xr-x");
        assert_eq!(format_permissions(0o041777), "drwxrwxrwt");
        assert_eq!(format_permissions(0o102644), "-rw-r-Sr--");
        assert_eq!(format_permissions(0o010600), "prw-------");
    }

    #[test]
    fn owner_and_group_come_from_the_servers_ls_line() {
        let attrs = FileAttributes { uid: Some(33), gid: Some(4), ..FileAttributes::empty() };
        let line = "-rw-r-----    1 www-data adm          1234 Nov 17 10:00 my file.log";
        assert_eq!(owner_group(line, &attrs), ("www-data".to_string(), "adm".to_string()));
    }

    #[test]
    fn without_an_ls_line_owner_and_group_are_the_ids() {
        let attrs = FileAttributes { uid: Some(1000), gid: Some(100), ..FileAttributes::empty() };
        assert_eq!(owner_group("", &attrs), ("1000".to_string(), "100".to_string()));
        assert_eq!(owner_group("", &FileAttributes::empty()), (String::new(), String::new()));
    }

    #[test]
    fn entries_join_their_folder_once() {
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/home/me", "a b"), "/home/me/a b");
        assert_eq!(join("/C:/Users/", "me"), "/C:/Users/me");
    }
}
