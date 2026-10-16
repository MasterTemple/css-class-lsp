use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};

const PATH_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

pub fn to_path(uri: &Uri) -> Option<PathBuf> {
    path_str(uri.as_str()).map(PathBuf::from)
}

pub fn from_path(path: &Path) -> Uri {
    Uri::from_str(&uri_str(&path.to_string_lossy())).expect("valid file uri")
}

/// `file:///C:/a` -> `C:/a` (Windows), `file:///a` -> `/a`.
fn path_str(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode_str(rest).decode_utf8().ok()?;
    match drive_path(&decoded) {
        Some(p) => Some(p.replace('/', "\\")),
        None => Some(decoded.into_owned()),
    }
}

/// `C:\a` -> `file:///C:/a`, `/a` -> `file:///a`.
fn uri_str(path: &str) -> String {
    if cfg!(windows) {
        let path = path.replace('\\', "/");
        let path = path.strip_prefix("//?/").unwrap_or(&path);
        let sep = if path.starts_with('/') { "" } else { "/" };
        format!("file://{sep}{}", utf8_percent_encode(path, PATH_SET))
    } else {
        format!("file://{}", utf8_percent_encode(path, PATH_SET))
    }
}

/// On Windows, the path part of `/C:/a` without the leading slash.
fn drive_path(p: &str) -> Option<&str> {
    let rest = p.strip_prefix('/')?;
    let b = rest.as_bytes();
    let is_drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    (cfg!(windows) && is_drive).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        if cfg!(windows) {
            assert_eq!(path_str("file:///C:/a%20b/c.rs").unwrap(), "C:\\a b\\c.rs");
            assert_eq!(path_str("file:///c%3A/a").unwrap(), "c:\\a");
            assert_eq!(uri_str("C:\\a b\\c.rs"), "file:///C:/a%20b/c.rs");
        } else {
            assert_eq!(path_str("file:///a%20b/c.rs").unwrap(), "/a b/c.rs");
            assert_eq!(uri_str("/a b/c.rs"), "file:///a%20b/c.rs");
        }
    }
}
