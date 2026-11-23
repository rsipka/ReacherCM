//! Reading an FTP server's folder listings: MLSD (RFC 3659), which says
//! what each entry is in so many words, and LIST, whose format is the
//! server's own: `ls -l` on Unix servers, `MM-DD-YY HH:MMAM <DIR>` on IIS.

use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::sftp::browser::{format_permissions, RemoteEntry};

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// The parent folder of a server path, or `/`.
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
    }
}

pub fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn skip(name: &str) -> bool {
    name.is_empty() || name == "." || name == ".."
}

/// One MLSD line: `type=file;size=12;modify=20261101120000; name`.
pub fn parse_mlsd_line(line: &str, dir: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (facts, name) = line.split_once(' ')?;
    if skip(name) {
        return None;
    }
    let mut is_dir = false;
    let mut size = 0;
    let mut modified = 0;
    let mut mode = None;
    let mut owner = String::new();
    let mut group = String::new();
    let mut link = false;
    for fact in facts.split(';').filter(|f| !f.is_empty()) {
        let Some((key, value)) = fact.split_once('=') else { continue };
        match key.to_ascii_lowercase().as_str() {
            "type" => {
                let v = value.to_ascii_lowercase();
                if v == "cdir" || v == "pdir" {
                    return None;
                }
                is_dir = v == "dir";
                // `OS.unix=slink:/target` and kin: a link, whose kind is unknown here.
                link = v.contains("slink") || v.contains("symlink");
            }
            "size" | "sizd" => size = value.parse().unwrap_or(0),
            "modify" => modified = mlsd_time(value).unwrap_or(0),
            "unix.mode" => mode = u32::from_str_radix(value.trim_start_matches("0o"), 8).ok(),
            "unix.owner" | "unix.ownername" => owner = value.to_string(),
            "unix.group" | "unix.groupname" => group = value.to_string(),
            // Ids only where the server has no names (pure-ftpd).
            "unix.uid" if owner.is_empty() => owner = value.to_string(),
            "unix.gid" if group.is_empty() => group = value.to_string(),
            _ => {}
        }
    }
    let kind_bits = if is_dir { 0o040000 } else if link { 0o120000 } else { 0o100000 };
    Some(RemoteEntry {
        name: name.to_string(),
        path: join(dir, name),
        is_dir,
        size: if is_dir { 0 } else { size },
        modified,
        permissions: mode.map(|m| format_permissions(kind_bits | (m & 0o7777))).unwrap_or_default(),
        owner,
        group,
    })
}

/// `YYYYMMDDHHMMSS[.sss]`, in UTC.
fn mlsd_time(value: &str) -> Option<u64> {
    let digits = value.split('.').next()?;
    let t = NaiveDateTime::parse_from_str(digits, "%Y%m%d%H%M%S").ok()?;
    u64::try_from(t.and_utc().timestamp()).ok()
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn month(token: &str) -> Option<u32> {
    let t = token.to_ascii_lowercase();
    MONTHS.iter().position(|m| t.starts_with(m) && t.len() <= 4).map(|i| i as u32 + 1)
}

/// The words of a line with where each starts, so a name keeps its spaces.
fn words(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push((s, &line[s..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, &line[s..]));
    }
    out
}

/// One `ls -l` line: `drwxr-xr-x 2 owner group 4096 Nov  1 12:00 name`.
/// `now` decides the year of a date without one (the last twelve months).
pub fn parse_unix_line(line: &str, dir: &str, now: chrono::DateTime<Utc>) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let w = words(line);
    let perms = w.first()?.1;
    if perms.len() < 10 || !perms.starts_with(|c: char| "-dlcbps".contains(c)) {
        return None;
    }
    // The date: a month, a day, then a time or a year.
    let at = (1..w.len().saturating_sub(3)).find(|&i| {
        month(w[i].1).is_some() && w[i + 1].1.parse::<u32>().is_ok_and(|d| (1..=31).contains(&d))
    })?;
    let mon = month(w[at].1)?;
    let day: u32 = w[at + 1].1.parse().ok()?;
    let when = w[at + 2].1;
    let name_start = w.get(at + 3)?.0;
    let mut name = &line[name_start..];
    let kind = perms.chars().next().unwrap_or('-');
    if kind == 'l' {
        if let Some((n, _target)) = name.split_once(" -> ") {
            name = n;
        }
    }
    if skip(name) {
        return None;
    }
    let size = w[at - 1].1.parse().unwrap_or(0);
    let modified = if let Some((h, m)) = when.split_once(':') {
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        let mut year = now.year();
        let mut t = NaiveDate::from_ymd_opt(year, mon, day)?.and_hms_opt(h, m, 0)?;
        // Without a year, `ls` means within the last six months or so: a
        // date ahead of now was last year.
        if Utc.from_utc_datetime(&t) > now + chrono::Duration::days(1) {
            year -= 1;
            t = NaiveDate::from_ymd_opt(year, mon, day)?.and_hms_opt(h, m, 0)?;
        }
        t.and_utc().timestamp()
    } else {
        let year: i32 = when.parse().ok()?;
        NaiveDate::from_ymd_opt(year, mon, day)?.and_hms_opt(0, 0, 0)?.and_utc().timestamp()
    };
    // Between the permissions and the size: links, owner and maybe group.
    let middle: Vec<&str> = w[1..at - 1].iter().map(|(_, s)| *s).collect();
    let (owner, group) = match middle.as_slice() {
        [_, o, g, ..] => (o.to_string(), g.to_string()),
        [_, o] => (o.to_string(), String::new()),
        _ => (String::new(), String::new()),
    };
    let is_dir = kind == 'd';
    Some(RemoteEntry {
        name: name.to_string(),
        path: join(dir, name),
        is_dir,
        size: if is_dir { 0 } else { size },
        modified: u64::try_from(modified).unwrap_or(0),
        permissions: perms.chars().take(10).collect(),
        owner,
        group,
    })
}

/// One IIS (DOS style) line: `11-01-26  12:00PM  <DIR>  name` or with a size.
pub fn parse_dos_line(line: &str, dir: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let w = words(line);
    if w.len() < 4 {
        return None;
    }
    let date = w[0].1;
    let time = w[1].1;
    let parts: Vec<&str> = date.split(['-', '/']).collect();
    let [m, d, y] = parts.as_slice() else { return None };
    let (m, d): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
    let mut y: i32 = y.parse().ok()?;
    if y < 100 {
        y += if y < 70 { 2000 } else { 1900 };
    }
    let upper = time.to_ascii_uppercase();
    let (clock, pm) = match upper.strip_suffix("PM") {
        Some(c) => (c.to_string(), Some(true)),
        None => match upper.strip_suffix("AM") {
            Some(c) => (c.to_string(), Some(false)),
            None => (upper.clone(), None),
        },
    };
    let (h, min) = clock.split_once(':')?;
    let (mut h, min): (u32, u32) = (h.parse().ok()?, min.parse().ok()?);
    match pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    let modified = NaiveDate::from_ymd_opt(y, m, d)?.and_hms_opt(h, min, 0)?.and_utc().timestamp();
    let is_dir = w[2].1.eq_ignore_ascii_case("<DIR>");
    let size = if is_dir { 0 } else { w[2].1.replace(',', "").parse().ok()? };
    let name = &line[w[3].0..];
    if skip(name) {
        return None;
    }
    Some(RemoteEntry {
        name: name.to_string(),
        path: join(dir, name),
        is_dir,
        size,
        modified: u64::try_from(modified).unwrap_or(0),
        permissions: String::new(),
        owner: String::new(),
        group: String::new(),
    })
}

pub fn parse_mlsd(text: &str, dir: &str) -> Vec<RemoteEntry> {
    text.lines().filter_map(|l| parse_mlsd_line(l, dir)).collect()
}

pub fn parse_list(text: &str, dir: &str) -> Vec<RemoteEntry> {
    let now = Utc::now();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("total "))
        .filter_map(|l| parse_unix_line(l, dir, now).or_else(|| parse_dos_line(l, dir)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
    }

    #[test]
    fn mlsd_lines() {
        let e = parse_mlsd_line("type=file;size=1234;modify=20261101120000.123;UNIX.mode=0644;UNIX.owner=web; index.html", "/srv").unwrap();
        assert_eq!((e.name.as_str(), e.path.as_str(), e.is_dir, e.size), ("index.html", "/srv/index.html", false, 1234));
        assert_eq!(e.modified, 1_793_534_400);
        assert_eq!(e.permissions, "-rw-r--r--");
        assert_eq!(e.owner, "web");
        let d = parse_mlsd_line("type=dir;modify=20261101120000; my folder", "/").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.path, "/my folder");
        assert!(parse_mlsd_line("type=cdir;modify=20261101120000; .", "/").is_none());
        assert!(parse_mlsd_line("type=pdir; ..", "/").is_none());
    }

    #[test]
    fn unix_lines() {
        let e = parse_unix_line("-rw-r--r--    1 ftp      ftp          5120 Nov 01 12:00 a  b.txt", "/pub", now()).unwrap();
        assert_eq!((e.name.as_str(), e.size, e.is_dir), ("a  b.txt", 5120, false));
        assert_eq!((e.owner.as_str(), e.group.as_str()), ("ftp", "ftp"));
        assert_eq!(e.modified, 1_793_534_400);
        // A date ahead of now is last year's.
        let old = parse_unix_line("-rw-r--r-- 1 u g 1 Dec 24 08:00 x", "/", now()).unwrap();
        assert_eq!(old.modified, Utc.with_ymd_and_hms(2025, 12, 24, 8, 0, 0).unwrap().timestamp() as u64);
        let y = parse_unix_line("drwxr-xr-x 2 u g 4096 Mar  3  2019 old", "/", now()).unwrap();
        assert!(y.is_dir);
        assert_eq!(y.modified, Utc.with_ymd_and_hms(2019, 3, 3, 0, 0, 0).unwrap().timestamp() as u64);
        let l = parse_unix_line("lrwxrwxrwx 1 u g 7 Nov 01 12:00 latest -> v1.2.3", "/", now()).unwrap();
        assert_eq!(l.name, "latest");
        // No group column.
        let ng = parse_unix_line("-rw-r--r-- 1 owner 42 Nov 01 12:00 n", "/", now()).unwrap();
        assert_eq!((ng.owner.as_str(), ng.size), ("owner", 42));
        assert!(parse_unix_line("total 12", "/", now()).is_none());
        assert!(parse_unix_line("drwxr-xr-x 2 u g 4096 Nov 01 12:00 .", "/", now()).is_none());
    }

    #[test]
    fn dos_lines() {
        let d = parse_dos_line("11-01-26  12:05PM       <DIR>          Web Site", "/").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.name, "Web Site");
        assert_eq!(d.modified, Utc.with_ymd_and_hms(2026, 11, 1, 12, 5, 0).unwrap().timestamp() as u64);
        let f = parse_dos_line("01-02-2024  12:30AM              1,234 a.txt", "/x").unwrap();
        assert_eq!((f.size, f.path.as_str()), (1234, "/x/a.txt"));
        assert_eq!(f.modified, Utc.with_ymd_and_hms(2024, 1, 2, 0, 30, 0).unwrap().timestamp() as u64);
    }

    #[test]
    fn whole_listings() {
        let unix = "total 8\r\ndrwxr-xr-x 2 u g 4096 Nov 01 12:00 .\r\ndrwxr-xr-x 2 u g 4096 Nov 01 12:00 sub\r\n-rw-r--r-- 1 u g 3 Nov 01 12:00 f\r\n";
        let names: Vec<_> = parse_list(unix, "/").into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["sub", "f"]);
    }

    #[test]
    fn paths() {
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/a/b/"), "/a");
        assert_eq!(base_name("/a/b c"), "b c");
    }
}
