use super::transfer::{CancellationToken, TransferError};
use std::path::{Path, PathBuf};

/// Keep partial downloads in their own sibling file. Dropping the writer on
/// any error (including task cancellation) removes just that staging file.
pub(crate) struct LocalDownload {
    pub(crate) writer: std::io::BufWriter<std::fs::File>,
    staging: tempfile::TempPath,
    destination: PathBuf,
}

impl LocalDownload {
    pub(crate) fn new(path: &Path) -> Result<Self, TransferError> {
        let prepare = || -> std::io::Result<Self> {
            // Keep the previous behavior of writing through a selected symlink.
            let destination = match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_symlink() => path.canonicalize()?,
                Ok(_) => path.to_owned(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => path.to_owned(),
                Err(e) => return Err(e),
            };
            let parent = destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            std::fs::create_dir_all(parent)?;
            let mut builder = tempfile::Builder::new();
            builder.prefix(".reach-download-");
            #[cfg(not(windows))]
            let temp = builder.tempfile_in(parent)?;
            #[cfg(windows)]
            let temp = windows_staging(&builder, parent)?;
            let (file, staging) = temp.into_parts();
            let download = Self {
                writer: std::io::BufWriter::with_capacity(1024 * 1024, file),
                staging,
                destination,
            };
            #[cfg(windows)]
            match std::fs::metadata(&download.destination) {
                Ok(meta) if meta.is_file() => {
                    copy_windows_dacl(&download.destination, download.writer.get_ref())?
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            Ok(download)
        };
        prepare().map_err(|e| {
            TransferError::IoError(format!(
                "Cannot prepare download to {}: {e}",
                path.display()
            ))
        })
    }

    pub(crate) fn commit(mut self, cancel: &CancellationToken) -> Result<(), TransferError> {
        use std::io::Write;
        let io_error = |e: std::io::Error| {
            TransferError::IoError(format!(
                "Cannot finish download to {}: {e}",
                self.destination.display()
            ))
        };
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        self.writer.flush().map_err(io_error)?;
        self.writer.get_ref().sync_all().map_err(io_error)?;
        // Atomic replacement requires write access to the directory. Also
        // respect restrictions on the original file, as an in-place write did.
        match std::fs::symlink_metadata(&self.destination) {
            Ok(meta) if meta.is_file() => {
                let original = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&self.destination)
                    .map_err(io_error)?;
                #[cfg(windows)]
                {
                    drop(original);
                    copy_windows_dacl(&self.destination, self.writer.get_ref())
                        .map_err(io_error)?;
                }
                #[cfg(unix)]
                copy_unix_access(&original, self.writer.get_ref()).map_err(io_error)?;
            }
            Ok(_) => {
                return Err(TransferError::IoError(format!(
                    "{} is not a regular file",
                    self.destination.display()
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
        drop(self.writer);
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        self.staging
            .persist(&self.destination)
            .map_err(|e| io_error(e.error))
    }
}

#[cfg(unix)]
fn copy_unix_access(original: &std::fs::File, replacement: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::{fchown, MetadataExt, PermissionsExt};
    let old = original.metadata()?;
    let new = replacement.metadata()?;
    if old.uid() != new.uid() || old.gid() != new.gid() {
        fchown(replacement, Some(old.uid()), Some(old.gid()))?;
    }
    // Keep rwx permissions but never transfer set-id privileges to new content.
    // Restore the ACL before chmod, so a default ACL inherited by staging
    // cannot gain rights when its group mask is widened.
    #[cfg(target_os = "linux")]
    {
        use xattr::FileExt;
        const ACL: &str = "system.posix_acl_access";
        match original.get_xattr(ACL) {
            Ok(Some(acl)) => replacement.set_xattr(ACL, &acl)?,
            Ok(None) if replacement.get_xattr(ACL)?.is_some() => replacement.remove_xattr(ACL)?,
            Ok(None) => {}
            Err(e) if e.raw_os_error() == Some(libc::ENOTSUP) => {}
            Err(e) => return Err(e),
        }
    }
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: both descriptors remain open. The ACL flag copies only access metadata.
        if unsafe {
            libc::fcopyfile(
                original.as_raw_fd(),
                replacement.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_ACL,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    replacement.set_permissions(std::fs::Permissions::from_mode(old.mode() & 0o777))
}

#[cfg(windows)]
fn windows_staging(
    builder: &tempfile::Builder<'_, '_>,
    parent: &Path,
) -> std::io::Result<tempfile::NamedTempFile> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::{Foundation::GENERIC_WRITE, Storage::FileSystem::WRITE_DAC};
    // Keep other processes out while receiving, including on shared folders.
    // WRITE_DAC lets us give the replacement the selected file's access list.
    builder.make_in(parent, |path| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .access_mode(GENERIC_WRITE.0 | WRITE_DAC.0)
            .open(path)
    })
}

#[cfg(windows)]
fn copy_windows_dacl(source: &Path, file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        SetKernelObjectSecurity, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the descriptor returned by Windows stays live through the set
    // call. The borrowed file owns the handle and we free the descriptor once.
    unsafe {
        let status = GetNamedSecurityInfoW(
            &HSTRING::from(source.as_os_str()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            &mut descriptor,
        );
        if status.is_err() {
            return Err(std::io::Error::from_raw_os_error(status.0 as i32));
        }
        let result = SetKernelObjectSecurity(
            HANDLE(file.as_raw_handle()),
            DACL_SECURITY_INFORMATION,
            descriptor,
        );
        LocalFree(Some(HLOCAL(descriptor.0)));
        result.map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn an_abandoned_download_cannot_remove_another_transfers_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same-name");
        let mut abandoned = LocalDownload::new(&path).unwrap();
        abandoned.writer.write_all(b"partial").unwrap();
        let mut completed = LocalDownload::new(&path).unwrap();
        completed.writer.write_all(b"finished").unwrap();
        completed.commit(&CancellationToken::new()).unwrap();
        drop(abandoned);
        assert_eq!(std::fs::read(path).unwrap(), b"finished");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn buffered_write_failure_preserves_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report");
        std::fs::write(&path, b"original").unwrap();
        let mut download = LocalDownload::new(&path).unwrap();
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        download.writer = std::io::BufWriter::new(full);
        download.writer.write_all(b"will fail on flush").unwrap();
        assert!(download.commit(&CancellationToken::new()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_symlink_target_keeps_the_link_and_mode() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("script");
        let link = dir.path().join("alias");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        symlink(&target, &link).unwrap();
        let mut download = LocalDownload::new(&link).unwrap();
        download.writer.write_all(b"new").unwrap();
        download.commit(&CancellationToken::new()).unwrap();
        assert!(link.is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_open_destination_that_denies_delete_survives_a_failed_replace() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("report");
        std::fs::write(&target, b"old").unwrap();
        let _reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .open(&target)
            .unwrap();
        let mut download = LocalDownload::new(&target).unwrap();
        download.writer.write_all(b"new").unwrap();
        assert!(download.commit(&CancellationToken::new()).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
