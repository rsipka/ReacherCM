//! Signatures checked against AWS's documented examples (their example keys).

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
