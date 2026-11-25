//! File-name patterns such as `SnapIt_%y-%mo-%d_%h-%mi-%s`.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default)]
pub struct Stamp {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

impl Stamp {
    pub fn now() -> Stamp {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Stamp { year: t.wYear, month: t.wMonth, day: t.wDay, hour: t.wHour, minute: t.wMinute, second: t.wSecond, millis: t.wMilliseconds }
    }
}

pub struct Vars<'a> {
    pub stamp: Stamp,
    pub width: u32,
    pub height: u32,
    pub title: &'a str,
}

fn random_token() -> String {
    // Not security sensitive; just avoids collisions in names.
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9)
        ^ (std::process::id() as u64) << 32;
    const ALPHA: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    (0..6)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ALPHA[(x % ALPHA.len() as u64) as usize] as char
        })
        .collect()
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    trimmed.chars().take(80).collect()
}

pub fn expand(pattern: &str, v: &Vars) -> String {
    let s = v.stamp;
    // Longest tokens first so `%mo`/`%mi`/`%ms` aren't eaten by shorter ones.
    let pairs: [(&str, String); 11] = [
        ("%height", v.height.to_string()),
        ("%width", v.width.to_string()),
        ("%title", sanitize(v.title)),
        ("%rnd", random_token()),
        ("%mo", format!("{:02}", s.month)),
        ("%mi", format!("{:02}", s.minute)),
        ("%ms", format!("{:03}", s.millis)),
        ("%y", format!("{:04}", s.year)),
        ("%d", format!("{:02}", s.day)),
        ("%h", format!("{:02}", s.hour)),
        ("%s", format!("{:02}", s.second)),
    ];
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        if rest.starts_with('%') {
            for (tok, val) in &pairs {
                if let Some(after) = rest.strip_prefix(tok) {
                    out.push_str(val);
                    rest = after;
                    continue 'outer;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    let name = sanitize(&out);
    if name.is_empty() { "SnapIt".into() } else { name }
}

/// `dir/name.ext`, adding ` (2)`, ` (3)` … if that file already exists.
pub fn unique_path(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{name}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{name} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(title: &str) -> Vars<'_> {
        Vars { stamp: Stamp { year: 2026, month: 3, day: 7, hour: 9, minute: 5, second: 4, millis: 12 }, width: 800, height: 600, title }
    }

    #[test]
    fn expands_tokens() {
        assert_eq!(expand("SnapIt_%y-%mo-%d_%h-%mi-%s", &vars("")), "SnapIt_2026-03-07_09-05-04");
        assert_eq!(expand("%width x%height.%ms", &vars("")), "800 x600.012");
        assert_eq!(expand("%title", &vars("a/b:c?")), "a_b_c_");
        assert_eq!(expand("%rnd", &vars("")).len(), 6);
        assert_eq!(expand("100%", &vars("")), "100%");
        assert_eq!(expand("%title", &vars("   ")), "SnapIt");
    }

    #[test]
    fn unique_paths() {
        let dir = std::env::temp_dir().join(format!("snapit-naming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "x", "png");
        std::fs::write(&a, b"").unwrap();
        let b = unique_path(&dir, "x", "png");
        assert!(b.ends_with("x (2).png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
