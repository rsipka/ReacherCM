use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path};

/// Validate a single filename supplied separately from its parent directory.
pub(crate) fn plain_name(name: &str) -> io::Result<&str> {
    let mut parts = Path::new(name).components();
    let normal = matches!(parts.next(), Some(Component::Normal(n)) if n == OsStr::new(name));
    if !normal || parts.next().is_some() || name.contains('\0') || !valid_platform_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid file or folder name",
        ));
    }
    Ok(name)
}

#[cfg(not(windows))]
fn valid_platform_name(_name: &str) -> bool {
    true
}

#[cfg(windows)]
fn valid_platform_name(name: &str) -> bool {
    if name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_ascii_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return false;
    }
    !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

/// Open the folder selected locally. All names below it are resolved through
/// directory handles, so a link supplied by a listing cannot redirect a write.
pub(crate) fn open_root(path: &Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}

pub(crate) fn download_subdir(root: &Dir, names: &[String]) -> io::Result<Dir> {
    for name in names {
        plain_name(name)?;
    }
    let mut dir = root.try_clone()?;
    for name in names {
        dir = match dir.open_dir_nofollow(name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match dir.create_dir(name) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
                dir.open_dir_nofollow(name)?
            }
            other => other?,
        };
        #[cfg(windows)]
        reject_reparse_point(&dir.dir_metadata()?)?;
    }
    Ok(dir)
}

#[cfg(windows)]
pub(crate) fn reject_reparse_point(metadata: &cap_std::fs::Metadata) -> io::Result<()> {
    use cap_fs_ext::OsMetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Download destination is a reparse point",
        ));
    }
    Ok(())
}

/// A newly created download that must be removed on failure. Keep the
/// directory handle for cleanup as well as publication.
pub(crate) struct PendingFile {
    pub(crate) dir: Dir,
    name: Option<OsString>,
}

impl PendingFile {
    pub(crate) fn new(dir: Dir, name: impl Into<OsString>) -> Self {
        Self {
            dir,
            name: Some(name.into()),
        }
    }

    pub(crate) fn keep(&mut self) {
        self.name = None;
    }

    pub(crate) fn replace(mut self, destination: &OsStr) -> io::Result<()> {
        let name = self.name.as_ref().expect("pending file");
        self.dir.rename(name, &self.dir, destination)?;
        self.keep();
        Ok(())
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if let Some(name) = self.name.take() {
            let _ = self.dir.remove_file(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_names_without_path_components() {
        for name in ["notes.txt", ".env", "name with spaces", "český.txt"] {
            assert_eq!(plain_name(name).unwrap(), name);
        }
        for name in ["", ".", "..", "/folder", "folder/child", "child/", "a\0b"] {
            assert!(plain_name(name).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_prefixes_and_special_names() {
        for name in [
            r"C:\folder",
            "D:notes.txt",
            "notes.txt:meta",
            "NUL.txt",
            "COM¹",
            "file.",
        ] {
            assert!(plain_name(name).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_a_link_when_opening_download_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("other")).unwrap();
        std::os::unix::fs::symlink("other", dir.path().join("linked")).unwrap();
        let root = open_root(dir.path()).unwrap();
        assert!(download_subdir(&root, &["linked".into()]).is_err());
        assert!(download_subdir(&root, &["inside".into(), "child".into()]).is_ok());
        assert!(dir.path().join("inside/child").is_dir());
    }
}
