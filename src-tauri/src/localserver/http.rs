//! HTTP: folder listings and downloads (with ranges, so downloads resume and
//! videos seek), and uploads with PUT when writing is allowed, from the
//! listing page's upload button or `curl -T`. Logins are HTTP Basic, which
//! travels in clear text like the rest of plain HTTP; the page says so.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures_util::StreamExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::fsroot::{io_reason, Root};
use super::Shared;

pub async fn run(listener: TcpListener, shared: Arc<Shared>, cancel: CancellationToken) {
    let app = Router::new().fallback(handle).with_state(shared.clone());
    let served = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(cancel.cancelled_owned())
        .await;
    if let Err(e) = served {
        shared.log(None, format!("Stopped: {e}"));
    }
}

fn text(status: StatusCode, body: &str) -> Response {
    let mut r = (status, body.to_string()).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

fn io_response(e: &std::io::Error) -> Response {
    let status = match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
        std::io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
        std::io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    text(status, io_reason(e))
}

/// The username a request logs in as, or None when it does not.
fn login(shared: &Shared, headers: &HeaderMap) -> Option<String> {
    use base64::Engine;
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ").or_else(|| value.strip_prefix("basic "))?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    shared.check_login(user, pass).then(|| user.to_string())
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if !shared.anonymous && login(&shared, req.headers()).is_none() {
        if req.headers().contains_key(header::AUTHORIZATION) {
            shared.log(Some(peer), "Login failed");
        }
        let mut r = text(StatusCode::UNAUTHORIZED, "Log in to see this folder");
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Reacher CM\", charset=\"UTF-8\""),
        );
        return r;
    }
    let raw_path = req.uri().path().to_string();
    let Some(decoded) = percent_decode(&raw_path) else {
        return text(StatusCode::BAD_REQUEST, "Bad path");
    };
    let virt = match Root::normalize("/", &decoded) {
        Ok(v) => v,
        Err(e) => return io_response(&e),
    };
    let real = match shared.root.real(&virt) {
        Ok(r) => r,
        Err(e) => return io_response(&e),
    };
    match *req.method() {
        Method::GET | Method::HEAD => {
            let head = req.method() == Method::HEAD;
            let meta = match fs::metadata(&real).await {
                Ok(m) => m,
                Err(e) => return io_response(&e),
            };
            if meta.is_dir() {
                if !raw_path.ends_with('/') {
                    let mut r = StatusCode::MOVED_PERMANENTLY.into_response();
                    if let Ok(v) = HeaderValue::from_str(&format!("{raw_path}/")) {
                        r.headers_mut().insert(header::LOCATION, v);
                    }
                    return r;
                }
                if !head {
                    shared.log(Some(peer), format!("List {virt}"));
                }
                return listing(&shared, &virt, &real, head).await;
            }
            serve_file(&shared, peer, &virt, &real, meta.len(), req.headers(), head).await
        }
        Method::PUT => {
            if !shared.allow_write {
                return text(StatusCode::FORBIDDEN, "This server is read only");
            }
            if virt == "/" || real.is_dir() {
                return text(StatusCode::CONFLICT, "Is a folder");
            }
            upload(&shared, peer, &virt, &real, req.into_body()).await
        }
        _ => {
            let mut r = text(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
            r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD, PUT"));
            r
        }
    }
}

async fn serve_file(
    shared: &Shared,
    peer: SocketAddr,
    virt: &str,
    real: &std::path::Path,
    size: u64,
    headers: &HeaderMap,
    head: bool,
) -> Response {
    let mut file = match fs::File::open(real).await {
        Ok(f) => f,
        Err(e) => return io_response(&e),
    };
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, size));
    let (status, start, len) = match range {
        Some(Ok((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        Some(Err(())) => {
            let mut r = text(StatusCode::RANGE_NOT_SATISFIABLE, "Range not satisfiable");
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return r;
        }
        None => (StatusCode::OK, 0, size),
    };
    if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "Seek failed");
    }
    if !head && start == 0 {
        shared.log(Some(peer), format!("Download {virt} ({size} bytes)"));
    }
    let body = if head {
        Body::empty()
    } else {
        Body::from_stream(tokio_util::io::ReaderStream::new(file.take(len)))
    };
    let mut r = Response::new(body);
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(virt)));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + len - 1)) {
            h.insert(header::CONTENT_RANGE, v);
        }
    }
    r
}

async fn upload(shared: &Shared, peer: SocketAddr, virt: &str, real: &std::path::Path, body: Body) -> Response {
    let mut file = match fs::File::create(real).await {
        Ok(f) => f,
        Err(e) => return io_response(&e),
    };
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = fs::remove_file(real).await;
                shared.log(Some(peer), format!("Upload of {virt} failed: {e}"));
                return text(StatusCode::BAD_REQUEST, "Upload interrupted");
            }
        };
        if let Err(e) = file.write_all(&chunk).await {
            drop(file);
            let _ = fs::remove_file(real).await;
            shared.log(Some(peer), format!("Upload of {virt} failed: {}", io_reason(&e)));
            return io_response(&e);
        }
        total += chunk.len() as u64;
    }
    if let Err(e) = file.flush().await {
        return io_response(&e);
    }
    shared.log(Some(peer), format!("Uploaded {virt} ({total} bytes)"));
    text(StatusCode::CREATED, "Uploaded")
}

/// One `bytes=` range: Ok((first, last)) inclusive, Err when it cannot be
/// satisfied, None for a header that is not a single byte range (served whole).
pub fn parse_range(value: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        let suffix: u64 = b.parse().ok()?;
        if suffix == 0 || size == 0 {
            return Some(Err(()));
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { size.saturating_sub(1) } else { b.parse::<u64>().ok()?.min(size.saturating_sub(1)) };
        if start >= size || end < start {
            return Some(Err(()));
        }
        (start, end)
    };
    Some(Ok(range))
}

pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

async fn listing(shared: &Shared, virt: &str, real: &std::path::Path, head: bool) -> Response {
    let mut dir = match fs::read_dir(real).await {
        Ok(d) => d,
        Err(e) => return io_response(&e),
    };
    let mut rows: Vec<(bool, String, u64, SystemTime)> = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if virt == "/" { format!("/{name}") } else { format!("{virt}/{name}") };
        if shared.root.real(&child).is_err() {
            continue;
        }
        let Ok(meta) = fs::metadata(entry.path()).await else { continue };
        rows.push((meta.is_dir(), name, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let title = html_escape(virt);
    let mut html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{title}</title><style>\
body{{font:14px system-ui,sans-serif;margin:16px;color:#222;background:#fff}}\
@media(prefers-color-scheme:dark){{body{{color:#ddd;background:#1b1b1f}}a{{color:#8ab4f8}}}}\
table{{border-collapse:collapse;width:100%;max-width:900px}}td,th{{padding:4px 8px;text-align:left;white-space:nowrap}}\
td.n{{white-space:normal;word-break:break-all;width:100%}}td.s{{text-align:right}}tr:hover{{background:rgba(127,127,127,.12)}}\
h1{{font-size:18px;word-break:break-all}}#up{{margin:12px 0}}small{{opacity:.7}}</style></head><body><h1>{title}</h1>"
    );
    if shared.allow_write {
        html.push_str(
            "<div id=\"up\"><input type=\"file\" id=\"f\" multiple> <button onclick=\"up()\">Upload</button> <small id=\"st\"></small></div>\
<script>async function up(){const fs=document.getElementById('f').files,st=document.getElementById('st');\
for(const f of fs){st.textContent=f.name+'…';const r=await fetch(encodeURIComponent(f.name),{method:'PUT',body:f});\
if(!r.ok){st.textContent=f.name+': '+await r.text();return}}location.reload()}</script>",
        );
    }
    html.push_str("<table><tr><th>Name</th><th>Size</th><th>Modified</th></tr>");
    if virt != "/" {
        html.push_str("<tr><td class=\"n\"><a href=\"../\">../</a></td><td></td><td></td></tr>");
    }
    for (is_dir, name, size, modified) in rows {
        let href = percent_encode(&name);
        let label = html_escape(&name);
        let when = chrono::DateTime::<chrono::Local>::from(modified).format("%Y-%m-%d %H:%M");
        if is_dir {
            html.push_str(&format!("<tr><td class=\"n\"><a href=\"{href}/\">{label}/</a></td><td></td><td>{when}</td></tr>"));
        } else {
            let size = human_size(size);
            html.push_str(&format!("<tr><td class=\"n\"><a href=\"{href}\">{label}</a></td><td class=\"s\">{size}</td><td>{when}</td></tr>"));
        }
    }
    html.push_str("</table><p><small>Reacher CM</small></p></body></html>");
    let len = html.len();
    let mut r = Response::new(if head { Body::empty() } else { Body::from(html) });
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    r
}

fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" | "log" | "md" | "conf" | "cfg" | "ini" | "sh" | "rsc" => "text/plain; charset=utf-8",
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        _ => "application/octet-stream",
    }
}
