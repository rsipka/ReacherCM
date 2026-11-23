//! A picture of one session pane (the terminal bar's camera button): the
//! page draws the PNG, and this puts it in a file or on the clipboard.
//! Desktop only.

use base64::Engine;
use std::time::Duration;

/// The largest picture taken in: a full 8K desktop is about 130 MB of RGBA,
/// far less as a PNG.
const MAX_PNG_BYTES: usize = 64 * 1024 * 1024;

fn decode(png_base64: &str) -> Result<Vec<u8>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64.as_bytes())
        .map_err(|e| format!("Not a picture: {e}"))?;
    if bytes.len() > MAX_PNG_BYTES {
        return Err("The picture is too large.".into());
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Not a PNG picture.".into());
    }
    Ok(bytes)
}

/// Write the picture to the file the save dialog chose.
#[tauri::command]
pub async fn screenshot_save(path: String, png_base64: String) -> Result<(), String> {
    let bytes = decode(&png_base64)?;
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Could not save the picture: {e}"))
}

/// Put the picture on the clipboard.
#[tauri::command]
pub async fn screenshot_copy(png_base64: String) -> Result<(), String> {
    let bytes = decode(&png_base64)?;
    let picture = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Not a picture: {e}"))?
        .into_rgba8();
    let (width, height) = picture.dimensions();
    let rgba = picture.into_raw();

    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        let mut clip = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
        };
        let image = arboard::ImageData { width: width as usize, height: height as usize, bytes: std::borrow::Cow::Owned(rgba) };
        // On Linux a picture is there only while its owner is: this thread
        // keeps it until something else is copied.
        #[cfg(target_os = "linux")]
        {
            use arboard::SetExtLinux;
            let _ = tx.send(Ok(()));
            if let Err(e) = clip.set().wait().image(image) {
                tracing::warn!("screenshot: clipboard: {e}");
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = tx.send(clip.set_image(image).map_err(|e| e.to_string()));
        }
    });
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "The clipboard did not answer.".to_string())?
        .map_err(|e| format!("Could not copy the picture: {e}"))
}
