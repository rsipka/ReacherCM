//! What the file manager does to an S3 session's buckets and objects: the
//! counterparts of `sftp::browser` and `sftp::transfer`, as `ftp::ops` is
//! for FTP.

use std::path::Path;

use base64::Engine;
use futures_util::StreamExt;
use md5::Digest as _;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::{split, valid_bucket, xml, Body, S3Conn, S3Error};
use crate::sftp::browser::RemoteEntry;
use crate::sftp::local_download::{DownloadTarget, LocalDownload};
use crate::sftp::transfer::plan_directory;

/// Hears a transfer's bytes done so far and its total.
pub type Progress<'a> = &'a (dyn Fn(u64, u64) + Send + Sync);

/// Largest object opened in the editor or viewer, as for SFTP.
const MAX_EDIT_SIZE: u64 = 5 * 1024 * 1024;
/// Files up to this size go up in one request; larger ones in parts.
pub(crate) const SINGLE_PUT_MAX: u64 = 16 * 1024 * 1024;
/// The smallest part; grown for very large files (S3 takes 10 000 parts).
const PART_SIZE: u64 = 16 * 1024 * 1024;
/// The most objects one folder listing reads.
const MAX_LIST: usize = 100_000;
/// The most objects a folder delete, move or rename takes on.
const MAX_TREE: usize = 100_000;

type Query = Vec<(String, String)>;

fn q(pairs: &[(&str, &str)]) -> Query {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn need_bucket(bucket: Option<String>, what: &str) -> Result<String, S3Error> {
    bucket.ok_or_else(|| S3Error::Other(format!("{what} needs a bucket: open one first")))
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn modified(text: Option<String>) -> u64 {
    text.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok()).map(|d| d.timestamp().max(0) as u64).unwrap_or(0)
}

fn entry(name: &str, path: String, is_dir: bool, size: u64, modified: u64) -> RemoteEntry {
    RemoteEntry {
        name: name.to_string(),
        path,
        is_dir,
        size,
        modified,
        permissions: String::new(),
        owner: String::new(),
        group: String::new(),
    }
}

fn sort(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

/// One page of keys and folders under `prefix`; `delimiter` `/` for one
/// level, none for everything below.
struct Page {
    objects: Vec<(String, u64, u64)>,
    prefixes: Vec<String>,
    next: Option<String>,
}

async fn list_page(conn: &S3Conn, bucket: &str, prefix: &str, delimiter: bool, token: Option<&str>) -> Result<Page, S3Error> {
    let mut query = q(&[("list-type", "2"), ("encoding-type", "url"), ("prefix", prefix)]);
    if delimiter {
        query.push(("delimiter".into(), "/".into()));
    }
    if let Some(t) = token {
        query.push(("continuation-token".into(), t.into()));
    }
    let body = conn.send(reqwest::Method::GET, Some(bucket), "", &query, Body::Empty, &[]).await?.text().await.map_err(|e| S3Error::Other(e.to_string()))?;
    let url = xml::text(&body, "EncodingType").is_some_and(|e| e.eq_ignore_ascii_case("url"));
    let decode = |s: String| if url { xml::url_decode(&s) } else { s };
    let objects = xml::blocks(&body, "Contents")
        .into_iter()
        .filter_map(|c| Some((decode(xml::text(c, "Key")?), xml::text(c, "Size").and_then(|s| s.parse().ok()).unwrap_or(0), modified(xml::text(c, "LastModified")))))
        .collect();
    let prefixes = xml::blocks(&body, "CommonPrefixes").into_iter().filter_map(|c| xml::text(c, "Prefix").map(decode)).collect();
    let truncated = xml::text(&body, "IsTruncated").is_some_and(|t| t == "true");
    let next = if truncated { xml::text(&body, "NextContinuationToken").filter(|t| !t.is_empty()) } else { None };
    Ok(Page { objects, prefixes, next })
}

pub async fn list_dir(conn: &S3Conn, path: &str) -> Result<Vec<RemoteEntry>, S3Error> {
    let (bucket, key) = split(path);
    let Some(bucket) = bucket else {
        let body = conn.send(reqwest::Method::GET, None, "", &[], Body::Empty, &[]).await?.text().await.map_err(|e| S3Error::Other(e.to_string()))?;
        let mut out: Vec<RemoteEntry> = xml::blocks(&body, "Bucket")
            .into_iter()
            .filter_map(|b| {
                let name = xml::text(b, "Name")?;
                Some(entry(&name, format!("/{name}"), true, 0, modified(xml::text(b, "CreationDate"))))
            })
            .collect();
        sort(&mut out);
        return Ok(out);
    };
    let prefix = if key.is_empty() { String::new() } else { format!("{key}/") };
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = list_page(conn, &bucket, &prefix, true, token.as_deref()).await?;
        for p in page.prefixes {
            let name = p[prefix.len().min(p.len())..].trim_end_matches('/').to_string();
            if !name.is_empty() {
                out.push(entry(&name, format!("/{bucket}/{}", p.trim_end_matches('/')), true, 0, 0));
            }
        }
        for (k, size, time) in page.objects {
            // The folder's own marker object.
            if k == prefix {
                continue;
            }
            let name = &k[prefix.len().min(k.len())..];
            out.push(entry(name, format!("/{bucket}/{k}"), false, size, time));
        }
        token = page.next;
        if token.is_none() || out.len() >= MAX_LIST {
            break;
        }
    }
    sort(&mut out);
    Ok(out)
}

/// Every key under `prefix` (which ends in `/`), with their sizes.
async fn list_tree(conn: &S3Conn, bucket: &str, prefix: &str) -> Result<Vec<(String, u64)>, S3Error> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = list_page(conn, bucket, prefix, false, token.as_deref()).await?;
        out.extend(page.objects.into_iter().map(|(k, s, _)| (k, s)));
        token = page.next;
        if out.len() > MAX_TREE {
            return Err(S3Error::Other(format!("{prefix} holds more than {MAX_TREE} objects; too many to do at once")));
        }
        if token.is_none() {
            return Ok(out);
        }
    }
}

/// The object's size, or None when there is no object of that key.
async fn head(conn: &S3Conn, bucket: &str, key: &str) -> Result<Option<u64>, S3Error> {
    match conn.send(reqwest::Method::HEAD, Some(bucket), key, &[], Body::Empty, &[]).await {
        Ok(r) => Ok(Some(r.content_length().unwrap_or(0))),
        Err(S3Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

async fn put(conn: &S3Conn, bucket: &str, key: &str, data: Vec<u8>) -> Result<(), S3Error> {
    conn.send(reqwest::Method::PUT, Some(bucket), key, &[], Body::Bytes(data.into()), &[]).await.map(|_| ())
}

/// A bucket at the root, else a folder (an empty `key/` object).
pub async fn make_dir(conn: &S3Conn, path: &str) -> Result<(), S3Error> {
    let (bucket, key) = split(path);
    let bucket = need_bucket(bucket, "A folder")?;
    if key.is_empty() {
        if !valid_bucket(&bucket) {
            return Err(S3Error::Other("A bucket name is 3 to 63 lower-case letters, digits, dots and hyphens".into()));
        }
        let region = conn.region();
        let body = if region == "us-east-1" {
            Body::Empty
        } else {
            Body::Bytes(format!("<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>", xml::escape(&region)).into_bytes().into())
        };
        return conn.send(reqwest::Method::PUT, Some(&bucket), "", &[], body, &[]).await.map(|_| ());
    }
    put(conn, &bucket, &format!("{key}/"), Vec::new()).await
}

/// Delete keys, a thousand to a request.
async fn delete_keys(conn: &S3Conn, bucket: &str, keys: &[String]) -> Result<(), S3Error> {
    for chunk in keys.chunks(1000) {
        let mut body = String::from("<Delete xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Quiet>true</Quiet>");
        for k in chunk {
            body.push_str(&format!("<Object><Key>{}</Key></Object>", xml::escape(k)));
        }
        body.push_str("</Delete>");
        let md5 = base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(body.as_bytes()));
        let answer = conn
            .send(reqwest::Method::POST, Some(bucket), "", &q(&[("delete", "")]), Body::Bytes(body.into_bytes().into()), &[("content-md5", md5), ("content-type", "application/xml".into())])
            .await?
            .text()
            .await
            .unwrap_or_default();
        if let Some(err) = xml::blocks(&answer, "Error").first() {
            let key = xml::text(err, "Key").unwrap_or_default();
            let msg = xml::text(err, "Message").or_else(|| xml::text(err, "Code")).unwrap_or_default();
            return Err(S3Error::Other(format!("Could not delete {key}: {msg}")));
        }
    }
    Ok(())
}

/// An object; a folder and all below it; or, at the root, an empty bucket.
pub async fn delete(conn: &S3Conn, path: &str) -> Result<(), S3Error> {
    let (bucket, key) = split(path);
    let bucket = need_bucket(bucket, "Deleting")?;
    if key.is_empty() {
        return conn.send(reqwest::Method::DELETE, Some(&bucket), "", &[], Body::Empty, &[]).await.map(|_| ());
    }
    let mut keys: Vec<String> = list_tree(conn, &bucket, &format!("{key}/")).await?.into_iter().map(|(k, _)| k).collect();
    if head(conn, &bucket, &key).await?.is_some() {
        keys.push(key);
    }
    delete_keys(conn, &bucket, &keys).await
}

async fn copy(conn: &S3Conn, from_bucket: &str, from: &str, to_bucket: &str, to: &str) -> Result<(), S3Error> {
    let source = format!("/{}/{}", super::sign::encode(from_bucket, false), super::encode_key(from));
    let answer = conn.send(reqwest::Method::PUT, Some(to_bucket), to, &[], Body::Empty, &[("x-amz-copy-source", source)]).await?.text().await.unwrap_or_default();
    // A copy can fail after its 200.
    if answer.contains("<Error>") {
        let (code, msg) = (xml::text(&answer, "Code").unwrap_or_default(), xml::text(&answer, "Message").unwrap_or_default());
        return Err(S3Error::Other(format!("Could not copy {from}: {code} {msg}")));
    }
    Ok(())
}

/// Rename (or move) an object, or a folder and all below it: each object
/// is copied, then the old ones deleted. Buckets are not renamed.
pub async fn rename(conn: &S3Conn, from: &str, to: &str) -> Result<(), S3Error> {
    let (fb, fk) = split(from);
    let (tb, tk) = split(to);
    let (fb, tb) = (need_bucket(fb, "Renaming")?, need_bucket(tb, "Renaming")?);
    if fk.is_empty() || tk.is_empty() {
        return Err(S3Error::Other("A bucket cannot be renamed".into()));
    }
    if fb == tb && (tk == fk || tk.starts_with(&format!("{fk}/"))) {
        return Err(S3Error::Other("A folder cannot be moved into itself".into()));
    }
    let mut moved = Vec::new();
    if head(conn, &fb, &fk).await?.is_some() {
        copy(conn, &fb, &fk, &tb, &tk).await?;
        moved.push(fk.clone());
    }
    let prefix = format!("{fk}/");
    for (k, _) in list_tree(conn, &fb, &prefix).await? {
        let dest = format!("{tk}/{}", &k[prefix.len()..]);
        copy(conn, &fb, &k, &tb, &dest).await?;
        moved.push(k);
    }
    if moved.is_empty() {
        return Err(S3Error::NotFound(format!("{from} does not exist")));
    }
    delete_keys(conn, &fb, &moved).await
}

/// An empty object, unless there is one of that name.
pub async fn touch(conn: &S3Conn, path: &str) -> Result<(), S3Error> {
    let (bucket, key) = split(path);
    let bucket = need_bucket(bucket, "A file")?;
    if key.is_empty() {
        return Err(S3Error::Other("A file needs a name inside a bucket".into()));
    }
    if head(conn, &bucket, &key).await?.is_none() {
        put(conn, &bucket, &key, Vec::new()).await?;
    }
    Ok(())
}

pub async fn read_text(conn: &S3Conn, path: &str) -> Result<String, S3Error> {
    let (bucket, key) = split(path);
    let bucket = need_bucket(bucket, "Reading")?;
    let size = head(conn, &bucket, &key).await?.ok_or_else(|| S3Error::NotFound(format!("{path} does not exist")))?;
    if size > MAX_EDIT_SIZE {
        return Err(S3Error::Other(format!("File too large to edit ({size} bytes, max {MAX_EDIT_SIZE})")));
    }
    let bytes = conn.send(reqwest::Method::GET, Some(&bucket), &key, &[], Body::Empty, &[]).await?.bytes().await.map_err(|e| S3Error::Other(e.to_string()))?;
    String::from_utf8(bytes.to_vec()).map_err(|_| S3Error::Other("File is not valid UTF-8 text".into()))
}

pub async fn write_text(conn: &S3Conn, path: &str, content: &str) -> Result<(), S3Error> {
    let (bucket, key) = split(path);
    let bucket = need_bucket(bucket, "Writing")?;
    put(conn, &bucket, &key, content.as_bytes().to_vec()).await
}

/// Download one object into `local`, through a staging file published only
/// once the whole object is there.
pub(crate) async fn download(conn: &S3Conn, remote: &str, local: &DownloadTarget, cancel: &CancellationToken, progress: Progress<'_>) -> Result<u64, S3Error> {
    let (bucket, key) = split(remote);
    let bucket = need_bucket(bucket, "Downloading")?;
    let response = conn.send(reqwest::Method::GET, Some(&bucket), &key, &[], Body::Empty, &[]).await?;
    let total = response.content_length();
    let mut file = LocalDownload::for_target(local).map_err(|e| S3Error::Other(e.to_string()))?;
    let mut stream = response.bytes_stream();
    let mut done: u64 = 0;
    let mut shown: u64 = 0;
    progress(0, total.unwrap_or(0));
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(S3Error::Cancelled),
            c = stream.next() => c,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| S3Error::Other(format!("{remote}: {e}")))?;
        std::io::Write::write_all(&mut file.writer, &chunk).map_err(|e| S3Error::Other(format!("Write error: {e}")))?;
        done += chunk.len() as u64;
        if done - shown >= 256 * 1024 {
            shown = done;
            progress(done, total.unwrap_or(done).max(done));
        }
    }
    if let Some(t) = total {
        if t != done {
            return Err(S3Error::Other(format!("{remote}: expected {t} bytes, received {done}")));
        }
    }
    file.commit(cancel).map_err(|e| match e {
        crate::sftp::transfer::TransferError::Cancelled => S3Error::Cancelled,
        other => S3Error::Other(other.to_string()),
    })?;
    progress(done, done);
    tracing::info!("S3 download complete: {remote} ({done} bytes)");
    Ok(done)
}

async fn read_chunk(file: &mut tokio::fs::File, len: usize) -> Result<Vec<u8>, S3Error> {
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let n = file.read(&mut buf[filled..]).await.map_err(|e| S3Error::Other(format!("Failed to read local file: {e}")))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Send one local file to `bucket/key`: in one request when small, else
/// in parts (aborted on a failure or cancel). `progress` hears bytes sent.
async fn send_file(conn: &S3Conn, local: &Path, bucket: &str, key: &str, cancel: &CancellationToken, progress: &(dyn Fn(u64) + Send + Sync)) -> Result<u64, S3Error> {
    let mut file = tokio::fs::File::open(local).await.map_err(|e| S3Error::Other(format!("Failed to read local file {}: {e}", local.display())))?;
    let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    if cancel.is_cancelled() {
        return Err(S3Error::Cancelled);
    }
    if size <= SINGLE_PUT_MAX {
        let data = read_chunk(&mut file, size as usize).await?;
        let n = data.len() as u64;
        put(conn, bucket, key, data).await?;
        progress(n);
        return Ok(n);
    }
    let part_size = PART_SIZE.max(size.div_ceil(9_000));
    let started = conn.send(reqwest::Method::POST, Some(bucket), key, &q(&[("uploads", "")]), Body::Empty, &[]).await?.text().await.unwrap_or_default();
    let upload_id = xml::text(&started, "UploadId").filter(|u| !u.is_empty()).ok_or_else(|| S3Error::Other("The store did not start a multipart upload".into()))?;
    let result = async {
        let mut parts = Vec::new();
        let mut sent: u64 = 0;
        let mut number = 1u32;
        loop {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let data = read_chunk(&mut file, part_size as usize).await?;
            if data.is_empty() {
                break;
            }
            let n = data.len() as u64;
            let query = q(&[("partNumber", &number.to_string()), ("uploadId", &upload_id)]);
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(S3Error::Cancelled),
                r = conn.send(reqwest::Method::PUT, Some(bucket), key, &query, Body::Bytes(data.into()), &[]) => r?,
            };
            let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            parts.push((number, etag));
            sent += n;
            progress(sent);
            number += 1;
        }
        let mut body = String::from("<CompleteMultipartUpload xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        for (n, etag) in &parts {
            body.push_str(&format!("<Part><PartNumber>{n}</PartNumber><ETag>{}</ETag></Part>", xml::escape(etag)));
        }
        body.push_str("</CompleteMultipartUpload>");
        let answer = conn.send(reqwest::Method::POST, Some(bucket), key, &q(&[("uploadId", &upload_id)]), Body::Bytes(body.into_bytes().into()), &[]).await?.text().await.unwrap_or_default();
        if answer.contains("<Error>") {
            return Err(S3Error::Other(format!("The store did not finish the upload: {}", xml::text(&answer, "Message").unwrap_or_default())));
        }
        Ok(sent)
    }
    .await;
    if result.is_err() {
        let _ = conn.send(reqwest::Method::DELETE, Some(bucket), key, &q(&[("uploadId", &upload_id)]), Body::Empty, &[]).await;
    }
    result
}

/// Upload a file, or a folder and all in it. A folder with nothing in it
/// gets a marker object so it shows. Cancellation stops between files and
/// parts.
pub async fn upload(conn: &S3Conn, local: &str, remote: &str, cancel: &CancellationToken, progress: Progress<'_>) -> Result<(), S3Error> {
    let (bucket, key) = split(remote);
    let bucket = need_bucket(bucket, "Uploading")?;
    if key.is_empty() {
        return Err(S3Error::Other("Files go inside a bucket, not beside the buckets".into()));
    }
    let path = Path::new(local);
    if path.is_dir() {
        let plan = plan_directory(path, remote).map_err(|e| S3Error::Other(format!("Failed to read local directory: {e}")))?;
        let total = plan.total_bytes;
        progress(0, total);
        for dir in &plan.dirs {
            let inside = format!("{}/", dir.trim_end_matches('/'));
            if !plan.files.iter().any(|f| f.remote.starts_with(&inside)) {
                make_dir(conn, dir).await?;
            }
        }
        if plan.files.is_empty() && plan.dirs.is_empty() {
            make_dir(conn, remote).await?;
        }
        let mut done: u64 = 0;
        for f in &plan.files {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let (fb, fk) = split(&f.remote);
            let fb = need_bucket(fb, "Uploading")?;
            let base = done;
            done += send_file(conn, &f.local, &fb, &fk, cancel, &|sent| progress(base + sent, total)).await?;
        }
        progress(total, total);
    } else {
        let total = std::fs::metadata(path).map(|m| m.len()).map_err(|e| S3Error::Other(format!("Failed to read local file {local}: {e}")))?;
        progress(0, total);
        let sent = send_file(conn, path, &bucket, &key, cancel, &|sent| progress(sent, total)).await?;
        progress(sent, sent);
    }
    tracing::info!("S3 upload complete: {}", base_name(remote));
    Ok(())
}
