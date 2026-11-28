//! Just enough XML for S3's answers: the elements of a name, their text,
//! with entities undone. S3's answers are flat and namespaced only on the
//! root, so this needs no full parser.

/// The inner text of each `<tag>…</tag>` in `body`, in order. A tag with
/// attributes (`<tag a="b">`) counts; a self-closed `<tag/>` is empty.
pub fn blocks<'a>(body: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = body[at..].find(&open) {
        let start = at + i + open.len();
        // `<tag>`, `<tag attr…>` or `<tag/>`, not `<tagger>`.
        let Some(next) = body[start..].chars().next() else { break };
        if !matches!(next, '>' | ' ' | '/' | '\t' | '\n' | '\r') {
            at = start;
            continue;
        }
        let Some(gt) = body[start..].find('>') else { break };
        let inner_start = start + gt + 1;
        if body[..inner_start].ends_with("/>") {
            out.push("");
            at = inner_start;
            continue;
        }
        let Some(end) = body[inner_start..].find(&close) else { break };
        out.push(&body[inner_start..inner_start + end]);
        at = inner_start + end + close.len();
    }
    out
}

/// The text of the first `<tag>` in `body`, entities undone.
pub fn text(body: &str, tag: &str) -> Option<String> {
    blocks(body, tag).first().map(|s| unescape(s))
}

/// `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` and numeric references.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest.find(';').filter(|n| *n <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// For a request body.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `%XX` undone, as keys come back with `encoding-type=url` (`+` is a space).
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(()) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_text() {
        let body = r#"<?xml version="1.0"?><ListBucketResult xmlns="x"><Name>b</Name><Contents><Key>a&amp;b</Key><Size>3</Size></Contents><Contents><Key>c</Key><Size>4</Size></Contents><CommonPrefixes><Prefix>d/</Prefix></CommonPrefixes><IsTruncated>false</IsTruncated><Empty/></ListBucketResult>"#;
        let c = blocks(body, "Contents");
        assert_eq!(c.len(), 2);
        assert_eq!(text(c[0], "Key").as_deref(), Some("a&b"));
        assert_eq!(text(c[1], "Size").as_deref(), Some("4"));
        assert_eq!(text(body, "Prefix").as_deref(), Some("d/"));
        assert_eq!(text(body, "Empty").as_deref(), Some(""));
        // <Name> is not <NameX>.
        assert_eq!(blocks("<NameX>1</NameX><Name>2</Name>", "Name"), vec!["2"]);
    }

    #[test]
    fn entities_and_urls() {
        assert_eq!(unescape("&lt;a&gt; &#x17E;&#382; & x"), "<a> žž & x");
        assert_eq!(url_decode("a%2Fb+c%C5%BE%"), "a/b cž%");
        assert_eq!(escape("a<&>"), "a&lt;&amp;&gt;");
    }
}
