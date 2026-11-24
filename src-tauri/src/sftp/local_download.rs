use super::transfer::{CancellationToken, TransferError};
use crate::local_paths::{download_subdir, open_root, plain_name, PendingFile};
#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// An explicit Save As path or a series of remote names under a locally selected folder.
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum DownloadTarget {
    Chosen(PathBuf),
    InFolder { root: PathBuf, names: Vec<String> },
}

impl DownloadTarget {
    pub(crate) fn path(&self) -> PathBuf {
        match self {
            Self::Chosen(path) => path.clone(),
            Self::InFolder { root, names } => root.join(names.iter().collect::<PathBuf>()),
        }
    }

    fn open_parent(&self) -> io::Result<(Dir, OsString)> {
        let (dir, name) = match self {
            Self::Chosen(path) => {
                // An explicit Save As path can intentionally point through a link.
                let path = match std::fs::symlink_metadata(path) {
                    Ok(meta) if meta.is_symlink() => path.canonicalize()?,
                    Ok(_) => path.clone(),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => path.clone(),
                    Err(e) => return Err(e),
                };
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let name = path.file_name().ok_or_else(invalid_destination)?;
                std::fs::create_dir_all(parent)?;
                (open_root(parent)?, name.to_owned())
            }
            Self::InFolder { root, names } => {
                let (name, parents) = names.split_last().ok_or_else(invalid_destination)?;
                let name = plain_name(name)?;
                (download_subdir(&open_root(root)?, parents)?, name.into())
            }
        };
        Ok((dir, name))
    }
}

fn invalid_destination() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "Download needs a file name")
}

/// Keep partial downloads in their own sibling file. Dropping the writer on
/// any error (including task cancellation) removes just that staging file.
pub(crate) struct LocalDownload {
    pub(crate) writer: std::io::BufWriter<std::fs::File>,
    staging: PendingFile,
    destination: OsString,
    display: PathBuf,
}

impl LocalDownload {
    #[cfg(test)]
    pub(crate) fn new(path: &Path) -> Result<Self, TransferError> {
        Self::for_target(&DownloadTarget::Chosen(path.to_owned()))
    }

    pub(crate) fn for_target(target: &DownloadTarget) -> Result<Self, TransferError> {
        let display = target.path();
        let prepare = || -> io::Result<Self> {
            let (dir, destination) = target.open_parent()?;
            // Check the destination before creating the staging file. Check it
            // again at commit, since a transfer can take minutes.
            let original = existing_file(&dir, &destination)?;
            let name = format!(".reach-download-{}", uuid::Uuid::new_v4());
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            #[cfg(windows)]
            {
                use windows::Win32::{Foundation::GENERIC_WRITE, Storage::FileSystem::WRITE_DAC};
                options
                    .share_mode(0)
                    .access_mode(GENERIC_WRITE.0 | WRITE_DAC.0);
            }
            let file = dir.open_with(&name, &options)?.into_std();
            let download = Self {
                writer: std::io::BufWriter::with_capacity(1024 * 1024, file),
                staging: PendingFile::new(dir, name),
                destination,
                display: display.clone(),
            };
            #[cfg(windows)]
            if let Some(original) = original {
                copy_windows_dacl(&original, download.writer.get_ref())?;
            }
            #[cfg(not(windows))]
            drop(original);
            Ok(download)
        };
        prepare().map_err(|e| {
            TransferError::IoError(format!(
                "Cannot prepare download to {}: {e}",
                display.display()
            ))
        })
    }

    pub(crate) fn commit(mut self, cancel: &CancellationToken) -> Result<(), TransferError> {
        use std::io::Write;
        let io_error = |e: io::Error| {
            TransferError::IoError(format!(
                "Cannot finish download to {}: {e}",
                self.display.display()
            ))
        };
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        self.writer.flush().map_err(io_error)?;
        self.writer.get_ref().sync_all().map_err(io_error)?;
        // Respect the original file's write permissions and copy its access
        // metadata. The original is opened without following links.
        if let Some(original) =
            existing_file(&self.staging.dir, &self.destination).map_err(io_error)?
        {
            #[cfg(windows)]
            copy_windows_dacl(&original, self.writer.get_ref()).map_err(io_error)?;
            #[cfg(unix)]
            copy_unix_access(&original, self.writer.get_ref()).map_err(io_error)?;
        }
        drop(self.writer);
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        self.staging.replace(&self.destination).map_err(io_error)
    }
}

fn existing_file(dir: &Dir, name: &std::ffi::OsStr) -> io::Result<Option<std::fs::File>> {
    let not_regular = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Destination is not a regular file",
        )
    };
    match dir.symlink_metadata(name) {
        Ok(metadata) if metadata.is_file() => {
            #[cfg(windows)]
            crate::local_paths::reject_reparse_point(&metadata)?;
        }
        Ok(_) => return Err(not_regular()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut options = OpenOptions::new();
    options.write(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    options.nonblock(true);
    #[cfg(windows)]
    {
        use windows::Win32::{Foundation::GENERIC_WRITE, Storage::FileSystem::READ_CONTROL};
        options.access_mode(GENERIC_WRITE.0 | READ_CONTROL.0);
    }
    let file = dir.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(not_regular());
    }
    #[cfg(windows)]
    crate::local_paths::reject_reparse_point(&metadata)?;
    Ok(Some(file.into_std()))
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
fn copy_windows_dacl(source: &std::fs::File, file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        SetKernelObjectSecurity, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the descriptor returned by Windows stays live through the set
    // call. The borrowed file owns the handle and we free the descriptor once.
    unsafe {
        let status = GetSecurityInfo(
            HANDLE(source.as_raw_handle()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut descriptor),
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

    #[cfg(windows)]
    #[test]
    fn windows_download_creates_publishes_and_cleans_up_in_the_selected_folder() {
        // Set this to a writable UNC share to exercise the network path.
        let root = match std::env::var_os("REACH_WINDOWS_TEST_SHARE") {
            Some(share) => tempfile::tempdir_in(share),
            None => tempfile::tempdir(),
        }.unwrap();
        let target = DownloadTarget::InFolder {
            root: root.path().to_owned(),
            names: vec!["nested".into(), "report.txt".into()],
        };
        let mut download = LocalDownload::for_target(&target).unwrap();
        download.writer.write_all(b"finished").unwrap();
        download.commit(&CancellationToken::new()).unwrap();
        drop(LocalDownload::for_target(&target).unwrap());

        assert_eq!(std::fs::read(target.path()).unwrap(), b"finished");
        assert_eq!(std::fs::read_dir(root.path().join("nested")).unwrap().count(), 1);
    }

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

    #[cfg(unix)]
    #[test]
    fn a_folder_download_refuses_a_destination_link_at_commit() {
        let dir = tempfile::tempdir().unwrap();
        let target = DownloadTarget::InFolder {
            root: dir.path().to_owned(),
            names: vec!["folder".into(), "report".into()],
        };
        let mut download = LocalDownload::for_target(&target).unwrap();
        download.writer.write_all(b"new").unwrap();

        let original = dir.path().join("original");
        std::fs::write(&original, b"original").unwrap();
        std::os::unix::fs::symlink(&original, target.path()).unwrap();
        assert!(download.commit(&CancellationToken::new()).is_err());
        assert!(target.path().is_symlink());
        assert_eq!(std::fs::read(original).unwrap(), b"original");
        assert_eq!(
            std::fs::read_dir(dir.path().join("folder"))
                .unwrap()
                .count(),
            1
        );
        assert!(LocalDownload::for_target(&target).is_err());
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
