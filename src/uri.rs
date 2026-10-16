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
    let rest = uri.as_str().strip_prefix("file://")?;
    let decoded = percent_decode_str(rest).decode_utf8().ok()?;
    Some(PathBuf::from(decoded.as_ref()))
}

pub fn from_path(path: &Path) -> Uri {
    let s = format!("file://{}", utf8_percent_encode(&path.to_string_lossy(), PATH_SET));
    Uri::from_str(&s).expect("valid file uri")
}
