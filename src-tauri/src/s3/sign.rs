//! AWS Signature Version 4, for requests (an `Authorization` header) and
//! for presigned links (the signature in the query). See AWS's "Signature
//! Version 4 signing process"; S3 signs the path as it is sent, without
//! encoding it a second time.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

/// The SHA-256 of nothing, the payload hash of a GET.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// What a presigned link's payload hash is.
pub const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// `%`-encoded as SigV4 wants: all but `A-Z a-z 0-9 - . _ ~`, and `/` too
/// unless `keep_slash`.
pub fn encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The query as signed and sent: names and values encoded, sorted by name.
pub fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query.iter().map(|(k, v)| (encode(k, false), encode(v, false))).collect();
    pairs.sort();
    pairs.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

pub struct Credentials<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
}

fn signing_key(c: &Credentials<'_>, date: &str) -> Vec<u8> {
    let mut key = hmac(format!("AWS4{}", c.secret_key).as_bytes(), date.as_bytes());
    for part in [c.region, "s3", "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    key
}

fn string_to_sign(amz_date: &str, scope: &str, canonical: &str) -> String {
    format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical.as_bytes()))
}

/// The `Authorization` header. `path` is already encoded; `headers` are
/// lower-case names and their values, `host`, `x-amz-content-sha256` and
/// `x-amz-date` among them.
pub fn authorization(c: &Credentials<'_>, method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload_sha256: &str, amz_date: &str) -> String {
    let mut headers: Vec<(&str, &str)> = headers.to_vec();
    headers.sort_by(|a, b| a.0.cmp(b.0));
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed = headers.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload_sha256}");
    let date = &amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", c.region);
    let signature = hex(&hmac(&signing_key(c, date), string_to_sign(amz_date, &scope, &canonical).as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", c.access_key)
}

/// The query of a presigned GET link for `path` on `host`, valid for
/// `expires` seconds from `amz_date`.
pub fn presigned_query(c: &Credentials<'_>, host: &str, path: &str, expires: u32, amz_date: &str) -> String {
    let date = &amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", c.region);
    let query = vec![
        ("X-Amz-Algorithm".to_string(), "AWS4-HMAC-SHA256".to_string()),
        ("X-Amz-Credential".to_string(), format!("{}/{scope}", c.access_key)),
        ("X-Amz-Date".to_string(), amz_date.to_string()),
        ("X-Amz-Expires".to_string(), expires.to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ];
    let q = canonical_query(&query);
    let canonical = format!("GET\n{path}\n{q}\nhost:{host}\n\nhost\n{UNSIGNED}");
    let signature = hex(&hmac(&signing_key(c, date), string_to_sign(amz_date, &scope, &canonical).as_bytes()));
    format!("{q}&X-Amz-Signature={signature}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDS: Credentials<'static> = Credentials {
        access_key: "AKIAIOSFODNN7EXAMPLE",
        secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        region: "us-east-1",
    };

    /// AWS's own example: GET Object with a Range header.
    #[test]
    fn get_object_example() {
        let auth = authorization(
            &CREDS,
            "GET",
            "/test.txt",
            "",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", EMPTY_SHA256),
                ("x-amz-date", "20130524T000000Z"),
            ],
            EMPTY_SHA256,
            "20130524T000000Z",
        );
        assert!(auth.ends_with("Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"), "{auth}");
        assert!(auth.contains("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date"));
    }

    /// AWS's own example: GET Bucket (List Objects) with a query.
    #[test]
    fn list_objects_example() {
        let q = canonical_query(&[("max-keys".into(), "2".into()), ("prefix".into(), "J".into())]);
        assert_eq!(q, "max-keys=2&prefix=J");
        let auth = authorization(
            &CREDS,
            "GET",
            "/",
            &q,
            &[("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", "20130524T000000Z")],
            EMPTY_SHA256,
            "20130524T000000Z",
        );
        assert!(auth.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"), "{auth}");
    }

    /// AWS's own example of a presigned URL.
    #[test]
    fn presigned_example() {
        let q = presigned_query(&CREDS, "examplebucket.s3.amazonaws.com", "/test.txt", 86400, "20130524T000000Z");
        assert!(q.ends_with("X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"), "{q}");
        assert!(q.contains("X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request"));
    }

    #[test]
    fn encoding() {
        assert_eq!(encode("a b/c+d~", true), "a%20b/c%2Bd~");
        assert_eq!(encode("a/b", false), "a%2Fb");
        assert_eq!(encode("ž", true), "%C5%BE");
    }
}
