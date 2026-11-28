//! An S3 store on this computer, over plain HTTP, that checks every
//! request's signature: buckets, listing in pages, folders, copies and
//! renames, deletes, a multipart upload, a download, a presigned link,
//! and a bucket in another region.

use super::*;
use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const ACCESS: &str = "AKIDEXAMPLE";
const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

#[derive(Default)]
struct Store {
    buckets: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    uploads: BTreeMap<String, BTreeMap<u32, Vec<u8>>>,
    requests: Vec<String>,
}

struct Req {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn q(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

async fn read_req(s: &mut tokio::net::TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (xml::url_decode(k), xml::url_decode(&v.replace('+', "%2B")))
        })
        .collect();
    Some(Req { method, path: path.to_string(), query, headers, body })
}

/// Whether the request is signed by our keys for `region`.
fn signed(r: &Req, region: &str) -> bool {
    if r.q("X-Amz-Signature").is_some() {
        let amz_date = r.q("X-Amz-Date").unwrap_or_default();
        let creds = sign::Credentials { access_key: ACCESS, secret_key: SECRET, region };
        let want = sign::presigned_query(&creds, r.header("host").unwrap_or_default(), &r.path, r.q("X-Amz-Expires").unwrap_or("0").parse().unwrap_or(0), amz_date);
        return want.ends_with(&format!("X-Amz-Signature={}", r.q("X-Amz-Signature").unwrap()));
    }
    let Some(auth) = r.header("authorization") else { return false };
    let signed_names = auth.split("SignedHeaders=").nth(1).and_then(|s| s.split(',').next()).unwrap_or_default();
    let pairs: Vec<(&str, &str)> = signed_names.split(';').map(|n| (n, r.header(n).unwrap_or_default())).collect();
    let payload = r.header("x-amz-content-sha256").unwrap_or_default();
    if payload != sign::sha256_hex(&r.body) {
        return false;
    }
    let query: Vec<(String, String)> = r.query.clone();
    let creds = sign::Credentials { access_key: ACCESS, secret_key: SECRET, region };
    let want = sign::authorization(&creds, &r.method, &r.path, &sign::canonical_query(&query), &pairs, payload, r.header("x-amz-date").unwrap_or_default());
    want == auth
}

fn respond(code: u16, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\nETag: \"e{}\"\r\nConnection: close\r\n\r\n{body}", body.len(), body.len()).into_bytes()
}

fn error(code: u16, s3: &str, extra: &str) -> Vec<u8> {
    respond(code, &format!("<?xml version=\"1.0\"?><Error><Code>{s3}</Code><Message>m</Message>{extra}</Error>"))
}

fn handle(store: &mut Store, r: Req) -> Vec<u8> {
    store.requests.push(format!("{} {}", r.method, r.path));
    let path = xml::url_decode(&r.path.replace('+', "%2B"));
    let p = path.trim_start_matches('/');
    let (bucket, key) = p.split_once('/').map(|(b, k)| (b.to_string(), k.to_string())).unwrap_or((p.to_string(), String::new()));
    let region = if bucket == "eu-bucket" { "eu-west-1" } else { "us-east-1" };
    if !signed(&r, region) {
        if bucket == "eu-bucket" && signed(&r, "us-east-1") {
            return error(400, "AuthorizationHeaderMalformed", "<Region>eu-west-1</Region>");
        }
        return error(403, "SignatureDoesNotMatch", "");
    }
    if bucket.is_empty() {
        let list: String = store.buckets.keys().map(|b| format!("<Bucket><Name>{b}</Name><CreationDate>2026-11-01T10:00:00.000Z</CreationDate></Bucket>")).collect();
        return respond(200, &format!("<ListAllMyBucketsResult><Buckets>{list}</Buckets></ListAllMyBucketsResult>"));
    }
    if r.method == "PUT" && key.is_empty() {
        store.buckets.entry(bucket).or_default();
        return respond(200, "");
    }
    if r.method == "DELETE" && key.is_empty() && r.q("uploadId").is_none() {
        return match store.buckets.get(&bucket) {
            Some(b) if !b.is_empty() => error(409, "BucketNotEmpty", ""),
            Some(_) => {
                store.buckets.remove(&bucket);
                respond(204, "")
            }
            None => error(404, "NoSuchBucket", ""),
        };
    }
    if !store.buckets.contains_key(&bucket) {
        return error(404, "NoSuchBucket", "");
    }
    if r.method == "GET" && key.is_empty() {
        let prefix = r.q("prefix").unwrap_or_default().to_string();
        let delim = r.q("delimiter").is_some();
        let start: usize = r.q("continuation-token").and_then(|t| t.parse().ok()).unwrap_or(0);
        let b = &store.buckets[&bucket];
        let mut items: Vec<(bool, String, usize)> = Vec::new();
        for (k, v) in b.range(prefix.clone()..) {
            if !k.starts_with(&prefix) {
                break;
            }
            let rest = &k[prefix.len()..];
            match rest.find('/').filter(|_| delim) {
                Some(i) => {
                    let pre = format!("{prefix}{}", &rest[..=i]);
                    if !items.iter().any(|(d, n, _)| *d && *n == pre) {
                        items.push((true, pre, 0));
                    }
                }
                None => items.push((false, k.clone(), v.len())),
            }
        }
        // Two to a page, to exercise continuation.
        let page: Vec<_> = items.iter().skip(start).take(2).collect();
        let more = start + 2 < items.len();
        let enc = |s: &str| sign::encode(s, true);
        let mut body = String::from("<ListBucketResult><EncodingType>url</EncodingType>");
        for (dir, name, size) in page {
            if *dir {
                body.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", enc(name)));
            } else {
                body.push_str(&format!("<Contents><Key>{}</Key><LastModified>2026-11-02T10:00:00.000Z</LastModified><Size>{size}</Size></Contents>", enc(name)));
            }
        }
        body.push_str(&format!("<IsTruncated>{more}</IsTruncated>"));
        if more {
            body.push_str(&format!("<NextContinuationToken>{}</NextContinuationToken>", start + 2));
        }
        body.push_str("</ListBucketResult>");
        return respond(200, &body);
    }
    if r.method == "POST" && r.q("delete").is_some() {
        let md5 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, <md5::Md5 as md5::Digest>::digest(&r.body));
        assert_eq!(r.header("content-md5"), Some(md5.as_str()));
        let text = String::from_utf8_lossy(&r.body).to_string();
        for o in xml::blocks(&text, "Object") {
            let k = xml::text(o, "Key").unwrap();
            store.buckets.get_mut(&bucket).unwrap().remove(&k);
        }
        return respond(200, "<DeleteResult/>");
    }
    if r.method == "POST" && r.q("uploads").is_some() {
        store.uploads.insert("up1".into(), BTreeMap::new());
        return respond(200, "<InitiateMultipartUploadResult><UploadId>up1</UploadId></InitiateMultipartUploadResult>");
    }
    if r.method == "PUT" && r.q("partNumber").is_some() {
        let n: u32 = r.q("partNumber").unwrap().parse().unwrap();
        store.uploads.get_mut(r.q("uploadId").unwrap()).unwrap().insert(n, r.body);
        return respond(200, "");
    }
    if r.method == "POST" && r.q("uploadId").is_some() {
        let parts = store.uploads.remove(r.q("uploadId").unwrap()).unwrap();
        let text = String::from_utf8_lossy(&r.body).to_string();
        assert_eq!(xml::blocks(&text, "Part").len(), parts.len());
        let data: Vec<u8> = parts.into_values().flatten().collect();
        store.buckets.get_mut(&bucket).unwrap().insert(key, data);
        return respond(200, "<CompleteMultipartUploadResult/>");
    }
    let b = store.buckets.get_mut(&bucket).unwrap();
    match r.method.as_str() {
        "PUT" => {
            if let Some(src) = r.header("x-amz-copy-source") {
                let src = xml::url_decode(&src.replace('+', "%2B"));
                let (sb, sk) = src.trim_start_matches('/').split_once('/').unwrap();
                assert_eq!(sb, bucket);
                let Some(data) = b.get(sk).cloned() else { return error(404, "NoSuchKey", "") };
                b.insert(key, data);
                return respond(200, "<CopyObjectResult/>");
            }
            b.insert(key, r.body);
            respond(200, "")
        }
        "HEAD" => match b.get(&key) {
            Some(v) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", v.len()).into_bytes(),
            None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        },
        "GET" => match b.get(&key) {
            Some(v) => {
                let mut out = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", v.len()).into_bytes();
                out.extend_from_slice(v);
                out
            }
            None => error(404, "NoSuchKey", ""),
        },
        "DELETE" => {
            b.remove(&key);
            respond(204, "")
        }
        _ => error(400, "Unknown", ""),
    }
}

async fn serve() -> (u16, Arc<tokio::sync::Mutex<Store>>) {
    // As run() does for the app.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = Arc::new(tokio::sync::Mutex::new(Store::default()));
    let st = store.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { return };
            let st = st.clone();
            tokio::spawn(async move {
                if let Some(r) = read_req(&mut s).await {
                    let out = handle(&mut *st.lock().await, r);
                    let _ = s.write_all(&out).await;
                    let _ = s.shutdown().await;
                }
            });
        }
    });
    (port, store)
}

fn params(port: u16) -> Params {
    Params {
        host: "127.0.0.1".into(),
        port,
        http: true,
        region: None,
        path_style: None,
        bucket: None,
        access_key: ACCESS.into(),
        secret_key: SECRET.into(),
        trusted: Vec::new(),
        socks: None,
    }
}

fn names(list: &[crate::sftp::browser::RemoteEntry]) -> Vec<String> {
    list.iter().map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" })).collect()
}

#[tokio::test]
async fn buckets_folders_and_files() {
    let (port, store) = serve().await;
    let conn = S3Conn::new(&params(port)).unwrap();
    conn.check().await.unwrap();
    assert_eq!(conn.home(), "/");

    ops::make_dir(&conn, "/photos").await.unwrap();
    ops::make_dir(&conn, "/backups").await.unwrap();
    assert_eq!(names(&ops::list_dir(&conn, "/").await.unwrap()), vec!["backups/", "photos/"]);
    assert!(ops::make_dir(&conn, "/Bad_Name").await.is_err());

    ops::make_dir(&conn, "/photos/2026").await.unwrap();
    ops::write_text(&conn, "/photos/2026/a b+c.txt", "hello").await.unwrap();
    ops::write_text(&conn, "/photos/2026/ž.txt", "ž").await.unwrap();
    ops::touch(&conn, "/photos/readme.md").await.unwrap();
    assert!(ops::touch(&conn, "/photos/x/../y").await.is_err());
    ops::touch(&conn, "/photos/x/y").await.unwrap();
    ops::write_text(&conn, "/photos/raw/1.cr2", "raw").await.unwrap();
    // Folders first; the folder's own marker is not listed in it; read in pages of two.
    assert_eq!(names(&ops::list_dir(&conn, "/photos").await.unwrap()), vec!["2026/", "raw/", "x/", "readme.md"]);
    let inner = ops::list_dir(&conn, "/photos/2026/").await.unwrap();
    assert_eq!(names(&inner), vec!["a b+c.txt", "ž.txt"]);
    assert_eq!(inner[0].path, "/photos/2026/a b+c.txt");
    assert_eq!(inner[0].size, 5);
    assert!(inner[0].modified > 0);
    assert_eq!(ops::read_text(&conn, "/photos/2026/ž.txt").await.unwrap(), "ž");

    // Rename a file, then a folder with all in it.
    ops::rename(&conn, "/photos/readme.md", "/photos/README.md").await.unwrap();
    ops::rename(&conn, "/photos/2026", "/photos/2025").await.unwrap();
    assert_eq!(names(&ops::list_dir(&conn, "/photos").await.unwrap()), vec!["2025/", "raw/", "x/", "README.md"]);
    assert_eq!(ops::read_text(&conn, "/photos/2025/a b+c.txt").await.unwrap(), "hello");
    assert!(ops::rename(&conn, "/photos/2025", "/photos/2025/in").await.is_err());

    // A folder goes with all below it; a bucket only when empty.
    ops::delete(&conn, "/photos/2025").await.unwrap();
    assert_eq!(names(&ops::list_dir(&conn, "/photos").await.unwrap()), vec!["raw/", "x/", "README.md"]);
    assert!(ops::delete(&conn, "/photos").await.is_err());
    ops::delete(&conn, "/backups").await.unwrap();
    assert_eq!(names(&ops::list_dir(&conn, "/").await.unwrap()), vec!["photos/"]);
    assert!(store.lock().await.requests.iter().any(|r| r.starts_with("POST")));
}

#[tokio::test]
async fn transfers_and_links() {
    let (port, store) = serve().await;
    let mut p = params(port);
    p.bucket = Some("data".into());
    let conn = S3Conn::new(&p).unwrap();
    ops::make_dir(&conn, "/data").await.unwrap();
    conn.check().await.unwrap();
    assert_eq!(conn.home(), "/data");

    let dir = tempfile::tempdir().unwrap();
    // Large enough for three parts.
    let big: Vec<u8> = (0..(ops::SINGLE_PUT_MAX as usize * 2 + 1000)).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.path().join("big.bin"), &big).unwrap();
    std::fs::create_dir_all(dir.path().join("tree/sub")).unwrap();
    std::fs::create_dir_all(dir.path().join("tree/empty")).unwrap();
    std::fs::write(dir.path().join("tree/sub/f.txt"), "f").unwrap();
    let cancel = CancellationToken::new();
    let seen = std::sync::Mutex::new(Vec::new());
    let progress = |done: u64, total: u64| seen.lock().unwrap().push((done, total));
    ops::upload(&conn, dir.path().join("big.bin").to_str().unwrap(), "/data/big.bin", &cancel, &progress).await.unwrap();
    assert_eq!(store.lock().await.buckets["data"]["big.bin"], big);
    assert!(seen.lock().unwrap().len() >= 4, "progress per part");
    ops::upload(&conn, dir.path().join("tree").to_str().unwrap(), "/data/tree", &cancel, &progress).await.unwrap();
    assert_eq!(names(&ops::list_dir(&conn, "/data/tree").await.unwrap()), vec!["empty/", "sub/"]);

    let target = crate::sftp::local_download::DownloadTarget::Chosen(dir.path().join("back.bin"));
    let n = ops::download(&conn, "/data/big.bin", &target, &cancel, &progress).await.unwrap();
    assert_eq!(n, big.len() as u64);
    assert_eq!(std::fs::read(dir.path().join("back.bin")).unwrap(), big);

    // A presigned link downloads without keys.
    let link = conn.presign("data", "tree/sub/f.txt", 600);
    assert!(link.starts_with(&format!("http://127.0.0.1:{port}/data/tree/sub/f.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256")));
    let body = reqwest::Client::new().get(&link).send().await.unwrap().text().await.unwrap();
    assert_eq!(body, "f");
}

#[tokio::test]
async fn wrong_keys_and_other_regions() {
    let (port, _store) = serve().await;
    let mut p = params(port);
    p.secret_key = "wrong".into();
    let bad = S3Conn::new(&p).unwrap();
    assert!(matches!(bad.check().await, Err(S3Error::Login(_))));

    let conn = S3Conn::new(&params(port)).unwrap();
    // The store names the bucket's region; the request goes again, signed for it.
    ops::make_dir(&conn, "/eu-bucket").await.unwrap();
    assert_eq!(conn.region(), "eu-west-1");
    ops::write_text(&conn, "/eu-bucket/a", "x").await.unwrap();
    assert!(matches!(ops::read_text(&conn, "/eu-bucket/missing").await, Err(S3Error::NotFound(_))));
}

#[test]
fn paths_and_addresses() {
    assert_eq!(split("/"), (None, String::new()));
    assert_eq!(split("/b"), (Some("b".into()), String::new()));
    assert_eq!(split("/b/a/c/"), (Some("b".into()), "a/c".into()));
    assert_eq!(encode_key("a b/../c"), "a%20b/%2E%2E/c");
    let mut p = params(9000);
    p.host = "https://S3.EU-Central-1.amazonaws.com/".into();
    p.http = false;
    p.port = 443;
    let aws = S3Conn::new(&p).unwrap();
    assert_eq!(aws.region(), "eu-central-1");
    assert_eq!(aws.target(Some("bkt"), "k"), ("bkt.s3.eu-central-1.amazonaws.com".into(), "/k".into()));
    assert_eq!(aws.target(Some("b.dot"), "k").1, "/b.dot/k");
    let mut p = params(9000);
    p.host = "minio.example.com".into();
    let minio = S3Conn::new(&p).unwrap();
    assert_eq!(minio.target(Some("bkt"), ""), ("minio.example.com:9000".into(), "/bkt".into()));
    assert!(minio.presign("bkt", "a.txt", 60).starts_with("http://minio.example.com:9000/bkt/a.txt?"));
    assert!(valid_bucket("my-bucket.1") && !valid_bucket("ab") && !valid_bucket("Upper"));
}
