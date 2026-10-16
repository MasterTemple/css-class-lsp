//! Finding class names used in source files (HTML, RSX, JSX, ...).

use std::sync::LazyLock;

use regex::Regex;

use crate::config::Pattern;

/// Byte ranges of strings that contain class lists.
pub fn class_regions(text: &str, patterns: &[Pattern]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for p in patterns {
        match p {
            Pattern::Single(re) => {
                for caps in re.captures_iter(text) {
                    out.extend(caps.iter().skip(1).flatten().map(|m| (m.start(), m.end())));
                }
            }
            Pattern::Nested(outer, inner) => {
                for caps in outer.captures_iter(text) {
                    let Some(region) = caps.get(1).or_else(|| caps.get(0)) else { continue };
                    for c in inner.captures_iter(region.as_str()) {
                        out.extend(
                            c.iter()
                                .skip(1)
                                .flatten()
                                .map(|m| (region.start() + m.start(), region.start() + m.end())),
                        );
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn regions_for(re: &Regex, text: &str) -> Vec<(usize, usize)> {
    re.captures_iter(text)
        .filter_map(|c| c.get(1))
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn is_separator(c: u8) -> bool {
    c.is_ascii_whitespace() || matches!(c, b'"' | b'\'' | b'`' | b'{' | b'}' | b'(' | b')' | b',' | b';')
}

/// Class-like tokens inside a region. `{...}` groups without quotes inside
/// (Rust format args like `"btn {extra}"`, `${expr}`) are skipped.
pub fn tokens(text: &str, (start, end): (usize, usize)) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        if b[i] == b'{' {
            if let Some(close) = b[i..end].iter().position(|&c| c == b'}') {
                let inner = &b[i + 1..i + close];
                if !inner.iter().any(|&c| matches!(c, b'"' | b'\'' | b'`' | b'{')) {
                    i += close + 1;
                    continue;
                }
            }
        }
        if is_separator(b[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < end && !is_separator(b[i]) {
            i += 1;
        }
        let mut e = i;
        // `{ foo: cond }` (Vue object syntax) and trailing `:` from ternaries.
        while e > s && b[e - 1] == b':' {
            e -= 1;
        }
        let tok = &text[s..e];
        if !tok.is_empty() && tok != "!important" && tok.bytes().any(|c| c.is_ascii_alphabetic()) {
            out.push((s, e));
        }
    }
    out
}

/// Byte offset where the class token under the cursor starts, if the cursor
/// is in a place where a class name is expected.
pub fn completion_start(text: &str, offset: usize, regions: &[(usize, usize)], css: bool) -> Option<usize> {
    static FALLBACK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\b(?:class|className)\s*[:=]\s*\{?\s*["'`][^"'`]*$"#).unwrap());
    static APPLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@apply\s[^;{}]*$").unwrap());

    let b = text.as_bytes();
    let token_start = |lo: usize| {
        let mut s = offset;
        while s > lo && !is_separator(b[s - 1]) {
            s -= 1;
        }
        s
    };
    if let Some(&(s, _)) = regions.iter().find(|&&(s, e)| s <= offset && offset <= e) {
        return Some(token_start(s));
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..offset];
    let re = if css { &*APPLY } else { &*FALLBACK };
    re.is_match(before).then(|| token_start(line_start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn found(src: &str) -> Vec<&str> {
        let cfg = Config::from_value(None);
        let mut v = Vec::new();
        for r in class_regions(src, &cfg.patterns) {
            v.extend(tokens(src, r).into_iter().map(|(s, e)| &src[s..e]));
        }
        v
    }

    #[test]
    fn rsx() {
        let src = r#"
rsx! {
    div { class: "card  card--big", "hello" }
    button { class: "btn {extra}", onclick: move |_| {} }
    span { class: if active { "on" } else { "off" } }
    p { class: format!("text-{} lead", size) }
}
"#;
        assert_eq!(found(src), ["card", "card--big", "btn", "on", "off", "text-", "lead"]);
    }

    #[test]
    fn html_jsx_vue() {
        let src = r#"<div class="a b"></div><X className={`c d`} /><p :class="{ 'e': x, f: y }"></p>
const k = clsx("g", cond && 'h');"#;
        assert_eq!(found(src), ["a", "b", "c", "d", "e", "x", "f", "y", "g", "h"]);
    }

    #[test]
    fn completion_context() {
        let src = "div { class: \"btn bt\" }";
        let cfg = Config::from_value(None);
        let regions = class_regions(src, &cfg.patterns);
        let off = src.find("bt\"").unwrap() + 2;
        assert_eq!(completion_start(src, off, &regions, false), Some(off - 2));
        assert_eq!(completion_start(src, 3, &regions, false), None);
        let unclosed = "div { class: \"bt";
        assert_eq!(completion_start(unclosed, unclosed.len(), &[], false), Some(unclosed.len() - 2));
    }
}
