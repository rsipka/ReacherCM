//! This device's own files, for the local side of the full-pane file
//! manager (the remote side is the SFTP explorer's). On a computer that is
//! the home folder and the drives. On a phone or tablet it starts in the
//! app's own folder there (see [`set_device_files_dir`]), which the system's
//! Files app shows too, so files can be passed in and out of it.
//!
//! The webview could already hand any local path to an upload, so this lets
//! it see nothing it could not already send; deleting and renaming are only
//! done on what the user picked, after the file manager has asked.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One entry of a local folder, in the shape the explorer's remote entries
/// have, so both panes sort and show alike.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "isDirectory")]
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
}

/// A local folder's contents, with the folder's own full path and its parent
/// (none at a root), worked out here so the page never has to know how
/// Windows writes a path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<LocalEntry>,
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn permissions_of(meta: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    crate::sftp::browser::format_permissions(meta.permissions().mode())
}

#[cfg(not(unix))]
fn permissions_of(meta: &std::fs::Metadata) -> String {
    let kind = if meta.is_dir() { 'd' } else { '-' };
    let write = if meta.permissions().readonly() { '-' } else { 'w' };
    format!("{kind}r{write}")
}

fn entry_of(path: &Path) -> Option<LocalEntry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    // A link is shown as what it points to; a broken one as itself.
    let meta = std::fs::metadata(path).or_else(|_| std::fs::symlink_metadata(path)).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(LocalEntry {
        name,
        path: path_string(path),
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        modified,
        permissions: permissions_of(&meta),
        owner: String::new(),
        group: String::new(),
    })
}

/// Where an empty path starts: the home folder on a computer, the app's
/// own files folder on a phone or tablet.
static DEVICE_FILES_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn home() -> Option<PathBuf> {
    DEVICE_FILES_DIR.get().cloned().or_else(dirs::home_dir)
}

/// The app's own files folder on a phone or tablet, made if missing:
/// - iPhone/iPad: the app's Documents, which the Files app shows under the
///   app's name (Info.ios.plist turns that on).
/// - Android: `Download/ReacherCM` in shared storage, which every file
///   manager shows; the app's private external folder when shared storage
///   cannot be written.
#[cfg(mobile)]
pub fn set_device_files_dir(app: &tauri::AppHandle) {
    use tauri::Manager;
    let paths = app.path();
    #[cfg(target_os = "ios")]
    let candidates = [paths.document_dir().ok()];
    #[cfg(target_os = "android")]
    let candidates = [
        paths.home_dir().ok().map(|h| h.join("Download").join("ReacherCM")),
        paths.download_dir().ok(),
    ];
    for dir in candidates.into_iter().flatten() {
        if std::fs::create_dir_all(&dir).is_ok() {
            tracing::info!("Device files folder: {}", dir.display());
            let _ = DEVICE_FILES_DIR.set(dir);
            return;
        }
    }
    tracing::warn!("No device files folder could be made");
}

pub(crate) fn list(path: &str) -> Result<LocalListing, String> {
    let dir = if path.is_empty() {
        home().ok_or_else(|| "No home folder".to_string())?
    } else {
        PathBuf::from(path)
    };
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", path_string(&dir)))?;
    let read = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", path_string(&dir)))?;
    let entries = read.filter_map(|e| e.ok()).filter_map(|e| entry_of(&e.path())).collect();
    Ok(LocalListing {
        path: display_path(&dir),
        parent: dir.parent().map(display_path),
        entries,
    })
}

/// Windows' canonical form starts `\\?\`; people write `C:\`.
fn display_path(p: &Path) -> String {
    let s = path_string(p);
    #[cfg(windows)]
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if !rest.starts_with("UNC\\") {
            return rest.to_string();
        }
    }
    s
}

/// A plain file or folder name: no separators, not `.` or `..`, so a name
/// typed into New folder or Rename can only land in the folder shown.
fn plain_name(name: &str) -> Result<&str, String> {
    crate::local_paths::plain_name(name).map_err(|_| format!("Not a valid name: {name}"))
}

/// Make a folder. With `exist_ok`, a folder already there is fine (a copy
/// into it merges); without, it is refused, as New folder should be.
pub(crate) fn mkdir(dir: &str, name: &str, exist_ok: bool) -> Result<String, String> {
    let target = Path::new(dir).join(plain_name(name)?);
    if exist_ok && target.is_dir() {
        return Ok(path_string(&target));
    }
    std::fs::create_dir(&target).map_err(|e| format!("{}: {e}", path_string(&target)))?;
    Ok(path_string(&target))
}

/// Make an empty file; a name already taken is refused.
pub(crate) fn touch(dir: &str, name: &str) -> Result<String, String> {
    let target = Path::new(dir).join(plain_name(name)?);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| format!("{}: {e}", path_string(&target)))?;
    Ok(path_string(&target))
}

pub(crate) fn rename(path: &str, new_name: &str) -> Result<String, String> {
    let from = Path::new(path);
    let parent = from.parent().ok_or_else(|| format!("Cannot rename {path}"))?;
    let to = parent.join(plain_name(new_name)?);
    if std::fs::symlink_metadata(&to).is_ok() {
        return Err(format!("{} already exists", path_string(&to)));
    }
    std::fs::rename(from, &to).map_err(|e| format!("{path}: {e}"))?;
    Ok(path_string(&to))
}

pub(crate) fn delete(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    // A link to a folder goes, not the folder it points to.
    let meta = std::fs::symlink_metadata(p).map_err(|e| format!("{path}: {e}"))?;
    let result = if meta.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    result.map_err(|e| format!("{path}: {e}"))
}

/// A child path of a local folder, as the OS writes it.
pub(crate) fn join(dir: &str, name: &str) -> Result<String, String> {
    Ok(path_string(&Path::new(dir).join(plain_name(name)?)))
}

/// Where a pane can start: the home folder (the app's files folder on a
/// phone), then each drive on Windows, the shared storage on Android, or
/// `/` on other computers.
fn roots() -> Vec<String> {
    let mut out: Vec<String> = home().map(|h| path_string(&h)).into_iter().collect();
    #[cfg(target_os = "android")]
    out.push("/storage/emulated/0".to_string());
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:\\", letter as char);
        if Path::new(&drive).exists() {
            out.push(drive);
        }
    }
    #[cfg(not(any(windows, mobile)))]
    out.push("/".to_string());
    out
}

/// The most the viewer and Compare read of one file, as the server side's
/// editor allows.
const MAX_TEXT: u64 = 5 * 1024 * 1024;

/// A local file as text, for the viewer and Compare. Refuses what is too
/// big to show or not text.
pub(crate) fn read_text(path: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{path}: is a folder"));
    }
    if meta.len() > MAX_TEXT {
        return Err(format!("File too large ({} bytes, max {MAX_TEXT})", meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8 text".to_string())
}

/// When a file last changed (milliseconds) and its size, so an edit in
/// another program can be noticed.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalStat {
    pub modified_ms: u64,
    pub size: u64,
}

pub(crate) fn stat(path: &str) -> Result<LocalStat, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(LocalStat { modified_ms, size: meta.len() })
}

/// A path for a server file opened in another program: `name` in a fresh
/// folder of its own under the temporary folder, so two files of the same
/// name never meet and the program shows the real name.
pub(crate) fn edit_copy_path(name: &str) -> Result<String, String> {
    let name = plain_name(name)?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("reachercm-edit-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let dir = builder.tempdir().map_err(|e| format!("Cannot create editor folder: {e}"))?.keep();
    Ok(path_string(&dir.join(name)))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// A local folder's contents; an empty path lists the home folder.
#[tauri::command]
pub async fn local_list_dir(path: String) -> Result<LocalListing, String> {
    blocking(move || list(&path)).await
}

#[tauri::command]
pub async fn local_mkdir(dir: String, name: String, exist_ok: Option<bool>) -> Result<String, String> {
    blocking(move || mkdir(&dir, &name, exist_ok.unwrap_or(false))).await
}

/// Create a server folder tree beneath the local pane's selected directory.
#[tauri::command]
pub async fn local_prepare_download_dir(root: String, names: Vec<String>) -> Result<(), String> {
    blocking(move || {
        let root = crate::local_paths::open_root(Path::new(&root)).map_err(|e| e.to_string())?;
        crate::local_paths::download_subdir(&root, &names).map(|_| ()).map_err(|e| e.to_string())
    }).await
}

/// Rename in place; refuses a name that is already taken.
#[tauri::command]
pub async fn local_rename(path: String, new_name: String) -> Result<String, String> {
    blocking(move || rename(&path, &new_name)).await
}

/// Delete a file, or a folder with everything in it.
#[tauri::command]
pub async fn local_delete(path: String) -> Result<(), String> {
    blocking(move || delete(&path)).await
}

#[tauri::command]
pub async fn local_touch(dir: String, name: String) -> Result<String, String> {
    blocking(move || touch(&dir, &name)).await
}

/// Show a file or folder in the system's file manager, selected.
#[cfg(desktop)]
#[tauri::command]
pub async fn local_reveal(path: String) -> Result<(), String> {
    if std::fs::symlink_metadata(&path).is_err() {
        return Err(format!("{path}: not found"));
    }
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn local_read_text(path: String) -> Result<String, String> {
    blocking(move || read_text(&path)).await
}

#[tauri::command]
pub async fn local_stat(path: String) -> Result<LocalStat, String> {
    blocking(move || stat(&path)).await
}

#[tauri::command]
pub async fn local_edit_copy_path(name: String) -> Result<String, String> {
    blocking(move || edit_copy_path(&name)).await
}

/// Open a local file in the program the system uses for it.
#[cfg(desktop)]
#[tauri::command]
pub async fn local_open(path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err(format!("{path}: not a file"));
    }
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn local_join(dir: String, name: String) -> Result<String, String> {
    join(&dir, &name)
}

#[tauri::command]
pub async fn local_roots() -> Result<Vec<String>, String> {
    Ok(roots())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_makes_renames_and_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = path_string(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();

        let made = mkdir(&dir, "sub", false).unwrap();
        assert!(mkdir(&dir, "sub", false).is_err(), "New folder refuses a taken name");
        assert_eq!(mkdir(&dir, "sub", true).unwrap(), made, "a copy merges into it");
        std::fs::write(Path::new(&made).join("inner"), b"x").unwrap();

        let listing = list(&dir).unwrap();
        let mut names: Vec<_> = listing.entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect();
        names.sort();
        assert_eq!(names, vec![("a.txt", false, 5), ("sub", true, 0)]);
        assert!(listing.parent.is_some());

        assert!(rename(&join(&dir, "a.txt").unwrap(), "sub").is_err(), "an existing name is refused");
        let renamed = rename(&join(&dir, "a.txt").unwrap(), "b.txt").unwrap();
        let empty = touch(&dir, "new.txt").unwrap();
        assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
        assert!(touch(&dir, "new.txt").is_err(), "New file refuses a taken name");
        assert!(Path::new(&renamed).exists());

        delete(&made).unwrap();
        assert!(!Path::new(&made).exists(), "a folder goes with what is in it");
    }

    #[test]
    fn reads_text_and_stats() {
        let tmp = tempfile::tempdir().unwrap();
        let text = tmp.path().join("t.txt");
        std::fs::write(&text, "ahoj\n").unwrap();
        assert_eq!(read_text(&path_string(&text)).unwrap(), "ahoj\n");
        assert_eq!(stat(&path_string(&text)).unwrap().size, 5);
        let bin = tmp.path().join("b.bin");
        std::fs::write(&bin, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_text(&path_string(&bin)).is_err());
        assert!(read_text(&path_string(tmp.path())).is_err(), "a folder is not read");
    }

    #[test]
    fn edit_copies_get_their_own_folder() {
        let a = edit_copy_path("nginx.conf").unwrap();
        let b = edit_copy_path("nginx.conf").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("nginx.conf"));
        assert!(edit_copy_path("../x").is_err());
        for p in [a, b] {
            let _ = std::fs::remove_dir(Path::new(&p).parent().unwrap());
        }
    }

    #[test]
    fn names_cannot_leave_the_folder() {
        for bad in ["", ".", "..", "../x", "a/b", "nul\0"] {
            assert!(plain_name(bad).is_err(), "{bad:?} accepted");
        }
        assert!(plain_name("report 2026.txt").is_ok());
    }

    #[test]
    fn root_has_no_parent() {
        #[cfg(not(windows))]
        assert_eq!(list("/").unwrap().parent, None);
    }
}
