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

/// A class-like token. `certain` is false when the token may not be a whole
/// class name: it touches an interpolation (`text-{size}`) or sits unquoted
/// inside an object/expression (`{ active: cond }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub certain: bool,
}

/// Class-like tokens inside a region. `{...}` groups without quotes inside
/// (Rust format args like `"btn {extra}"`, `${expr}`) are skipped.
pub fn tokens(text: &str, (start, end): (usize, usize)) -> Vec<Token> {
    let b = text.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = start;
    while i < end {
        let c = b[i];
        if c == b'{' {
            if let Some(close) = b[i..end].iter().position(|&c| c == b'}') {
                let inner = &b[i + 1..i + close];
                if !inner.iter().any(|&c| matches!(c, b'"' | b'\'' | b'`' | b'{')) {
                    // Interpolation: whatever touches it is only part of a name.
                    if let Some(last) = out.last_mut().filter(|t| t.end == i) {
                        last.certain = false;
                    }
                    // Drop a fragment glued to the end of it (`{a}-x`).
                    i += close + 1;
                    while i < end && !is_separator(b[i]) {
                        i += 1;
                    }
                    continue;
                }
            }
            depth += 1;
        } else if c == b'}' {
            depth = depth.saturating_sub(1);
        } else if depth > 0 && matches!(c, b'"' | b'\'' | b'`') {
            quote = match quote {
                Some(q) if q == c => None,
                None => Some(c),
                q => q,
            };
        }
        if is_separator(c) {
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
            out.push(Token { start: s, end: e, certain: depth == 0 || quote.is_some() });
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
            v.extend(tokens(src, r).into_iter().map(|t| &src[t.start..t.end]));
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
    fn uncertain_tokens() {
        let src = r#"div { class: "text-{size} {a}-x lead" } <p :class="{ 'e': x }"></p>"#;
        let cfg = Config::from_value(None);
        let toks: Vec<(&str, bool)> = class_regions(src, &cfg.patterns)
            .into_iter()
            .flat_map(|r| tokens(src, r))
            .map(|t| (&src[t.start..t.end], t.certain))
            .collect();
        assert_eq!(toks, [("text-", false), ("lead", true), ("e", true), ("x", false)]);
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
