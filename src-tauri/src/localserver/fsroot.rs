//! The served folder, and the one way a client's path becomes a path on disk.
//!
//! Clients name paths as if the folder were the whole file system: `/` is the
//! folder itself. `..` stops at `/`, both `/` and `\` separate names (TFTP
//! clients on Windows send backslashes), and a symbolic link inside the
//! folder that leads outside it is refused, so no path a client can send
//! reaches a file the folder does not hold.

use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Root {
    /// Canonical: symbolic links resolved, absolute.
    path: PathBuf,
}

impl Root {
    pub fn new(folder: &str) -> Result<Self, String> {
        let folder = folder.trim();
        if folder.is_empty() {
            return Err("Choose a folder to serve".into());
        }
        let path = std::fs::canonicalize(folder).map_err(|e| format!("{folder}: {e}"))?;
        if !path.is_dir() {
            return Err(format!("{folder} is not a folder"));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A client's path, relative to the virtual directory `cwd`, as a clean
    /// absolute virtual path: `/`, `/a`, `/a/b`.
    pub fn normalize(cwd: &str, path: &str) -> Result<String, io::Error> {
        if path.contains('\0') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name"));
        }
        let path = path.replace('\\', "/");
        let mut parts: Vec<&str> = Vec::new();
        let start = if path.starts_with('/') { "" } else { cwd };
        for part in start.split('/').chain(path.split('/')) {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                name => {
                    // A drive letter or a stream name means nothing here and
                    // must not be handed to Windows as one.
                    if cfg!(windows) && name.contains(':') {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name"));
                    }
                    parts.push(name);
                }
            }
        }
        Ok(format!("/{}", parts.join("/")))
    }

    /// The file on disk a virtual path names. Whatever part of it exists is
    /// resolved and must still be inside the folder.
    pub fn real(&self, virtual_path: &str) -> io::Result<PathBuf> {
        let clean = Self::normalize("/", virtual_path)?;
        let mut real = self.path.clone();
        for part in clean.split('/').filter(|p| !p.is_empty()) {
            // normalize() leaves only plain names; a name that is still a
            // root or prefix to Path (it cannot be on Unix) is refused.
            if !matches!(Path::new(part).components().next(), Some(Component::Normal(_)))
                || Path::new(part).components().count() != 1
            {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name"));
            }
            real.push(part);
        }
        self.check_inside(&real)?;
        Ok(real)
    }

    /// Refuses a path whose existing part resolves outside the folder (a
    /// symbolic link to elsewhere).
    fn check_inside(&self, real: &Path) -> io::Result<()> {
        let mut existing = real;
        loop {
            match std::fs::canonicalize(existing) {
                Ok(resolved) => {
                    return if resolved.starts_with(&self.path) {
                        Ok(())
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "Outside the served folder",
                        ))
                    };
                }
                Err(_) => match existing.parent() {
                    Some(parent) => existing = parent,
                    None => return Ok(()),
                },
            }
        }
    }

    /// The virtual path of a file on disk inside the folder.
    pub fn virtual_of(&self, real: &Path) -> String {
        match real.strip_prefix(&self.path) {
            Ok(rel) => {
                let parts: Vec<String> = rel
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect();
                format!("/{}", parts.join("/"))
            }
            Err(_) => "/".into(),
        }
    }
}

/// A short reason for a failed file operation, for a client and the log.
pub fn io_reason(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::NotFound => "No such file or folder",
        io::ErrorKind::PermissionDenied => "Permission denied",
        io::ErrorKind::AlreadyExists => "Already exists",
        io::ErrorKind::InvalidInput => "Invalid file name",
        io::ErrorKind::DirectoryNotEmpty => "Folder is not empty",
        io::ErrorKind::IsADirectory => "Is a folder",
        io::ErrorKind::NotADirectory => "Not a folder",
        io::ErrorKind::StorageFull => "Disk full",
        _ => "Failed",
    }
}
