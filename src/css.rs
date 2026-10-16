//! A forgiving CSS/SCSS/LESS scanner. It does not validate anything; it only
//! finds class selectors in rule preludes and the extent of their rule blocks.

/// A class found in a selector. All offsets are absolute byte offsets.
#[derive(Debug, Clone)]
pub struct RawDef {
    pub name: String,
    /// Span of the token in the selector (includes the leading `.` or `&`).
    pub start: usize,
    pub end: usize,
    /// The whole rule, from the start of the selector to the closing brace.
    pub rule_start: usize,
    pub rule_end: usize,
}

struct Frame {
    /// Classes of this rule's selector, used to resolve `&-suffix` in children.
    classes: Vec<String>,
    defs: Vec<usize>,
}

/// Scan `text[start..end]`. `scss` enables `//` comments and `#{}` interpolation.
pub fn parse(text: &str, start: usize, end: usize, scss: bool) -> Vec<RawDef> {
    let b = text.as_bytes();
    let mut defs: Vec<RawDef> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut seg = start;
    let mut i = start;

    while i < end {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = skip_block_comment(b, i, end);
                continue;
            }
            b'/' if scss && b.get(i + 1) == Some(&b'/') && (i == 0 || b[i - 1] != b':') => {
                i = memchr(b, b'\n', i, end);
                continue;
            }
            b'"' | b'\'' => {
                i = skip_string(b, i, end);
                continue;
            }
            b'#' if scss && b.get(i + 1) == Some(&b'{') => {
                i = skip_balanced(b, i + 1, end);
                continue;
            }
            b'{' => {
                let parent = stack.last().map(|f| f.classes.as_slice()).unwrap_or(&[]);
                let frame = prelude(text, seg, i, parent, scss, &mut defs);
                stack.push(frame);
                seg = i + 1;
            }
            b'}' => {
                if let Some(f) = stack.pop() {
                    for d in f.defs {
                        defs[d].rule_end = i + 1;
                    }
                }
                seg = i + 1;
            }
            b';' => seg = i + 1,
            _ => {}
        }
        i += 1;
    }
    for f in stack {
        for d in f.defs {
            defs[d].rule_end = end;
        }
    }
    defs
}

fn prelude(text: &str, seg: usize, brace: usize, parent: &[String], scss: bool, defs: &mut Vec<RawDef>) -> Frame {
    let b = text.as_bytes();
    let lead = skip_trivia(b, seg, brace, scss);
    let mut frame = Frame { classes: Vec::new(), defs: Vec::new() };
    let mut push = |name: String, s: usize, e: usize, frame: &mut Frame| {
        if frame.classes.contains(&name) {
            return;
        }
        frame.classes.push(name.clone());
        frame.defs.push(defs.len());
        defs.push(RawDef { name, start: s, end: e, rule_start: lead, rule_end: brace + 1 });
    };

    if b.get(lead) == Some(&b'@') {
        // Tailwind v4: `@utility name { ... }` defines a class.
        let rest = &text[lead..brace];
        if let Some(after) = rest.strip_prefix("@utility") {
            let off = lead + "@utility".len() + (after.len() - after.trim_start().len());
            if let Some((name, e)) = read_ident(text, off, brace) {
                push(name, off, e, &mut frame);
            }
        } else {
            // @media / @layer / @supports ...: children still nest under the parent rule.
            frame.classes = parent.to_vec();
        }
        return frame;
    }

    let mut j = lead;
    let mut saw_amp = false;
    while j < brace {
        match b[j] {
            b'/' if b.get(j + 1) == Some(&b'*') => {
                j = skip_block_comment(b, j, brace);
                continue;
            }
            b'/' if scss && b.get(j + 1) == Some(&b'/') => {
                j = memchr(b, b'\n', j, brace);
                continue;
            }
            b'"' | b'\'' => {
                j = skip_string(b, j, brace);
                continue;
            }
            b'[' => {
                // Attribute selectors: [href$=".css"]
                j += 1;
                while j < brace && b[j] != b']' {
                    j = if b[j] == b'"' || b[j] == b'\'' { skip_string(b, j, brace) } else { j + 1 };
                }
            }
            b'.' => {
                if let Some((name, e)) = read_ident(text, j + 1, brace) {
                    push(name, j, e, &mut frame);
                    j = e;
                    continue;
                }
            }
            b'&' => {
                saw_amp = true;
                let mut e = j + 1;
                while e < brace && (b[e].is_ascii_alphanumeric() || b[e] == b'-' || b[e] == b'_') {
                    e += 1;
                }
                if e > j + 1 {
                    let suffix = &text[j + 1..e];
                    for p in parent {
                        push(format!("{p}{suffix}"), j, e, &mut frame);
                    }
                    j = e;
                    continue;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if frame.classes.is_empty() && saw_amp {
        frame.classes = parent.to_vec();
    }
    frame
}

/// Read a CSS identifier starting at `start`, decoding escapes (`.md\:flex` -> `md:flex`).
pub fn read_ident(text: &str, start: usize, end: usize) -> Option<(String, usize)> {
    let b = text.as_bytes();
    let first = *b.get(start)?;
    let second = b.get(start + 1).copied().unwrap_or(0);
    let starts_ok = |c: u8| c.is_ascii_alphabetic() || c == b'_' || c == b'\\' || c >= 0x80;
    if !(starts_ok(first) || (first == b'-' && (starts_ok(second) || second == b'-'))) {
        return None;
    }
    let mut name = String::new();
    let mut j = start;
    while j < end {
        let c = b[j];
        if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
            name.push(c as char);
            j += 1;
        } else if c >= 0x80 {
            let ch = text[j..].chars().next()?;
            name.push(ch);
            j += ch.len_utf8();
        } else if c == b'\\' && j + 1 < end && b[j + 1] != b'\n' {
            let hex_len = b[j + 1..end.min(j + 7)].iter().take_while(|c| c.is_ascii_hexdigit()).count();
            if hex_len > 0 {
                let code = u32::from_str_radix(&text[j + 1..j + 1 + hex_len], 16).ok()?;
                name.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                j += 1 + hex_len;
                if j < end && b[j].is_ascii_whitespace() {
                    j += 1;
                }
            } else {
                let ch = text[j + 1..].chars().next()?;
                name.push(ch);
                j += 1 + ch.len_utf8();
            }
        } else {
            break;
        }
    }
    (!name.is_empty() && name != "-").then_some((name, j))
}

fn skip_trivia(b: &[u8], mut i: usize, end: usize, scss: bool) -> usize {
    while i < end {
        if b[i].is_ascii_whitespace() {
            i += 1;
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(b, i, end);
        } else if scss && b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            i = memchr(b, b'\n', i, end);
        } else {
            break;
        }
    }
    i
}

fn skip_block_comment(b: &[u8], i: usize, end: usize) -> usize {
    let mut j = i + 2;
    while j + 1 < end {
        if b[j] == b'*' && b[j + 1] == b'/' {
            return j + 2;
        }
        j += 1;
    }
    end
}

fn skip_string(b: &[u8], i: usize, end: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < end {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            b'\n' => return j, // unterminated
            _ => j += 1,
        }
    }
    end
}

fn skip_balanced(b: &[u8], open: usize, end: usize) -> usize {
    let mut depth = 0;
    let mut j = open;
    while j < end {
        match b[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    end
}

fn memchr(b: &[u8], c: u8, i: usize, end: usize) -> usize {
    b[i..end].iter().position(|&x| x == c).map_or(end, |p| i + p)
}

/// Rule text for previews: dedented and truncated.
pub fn snippet(text: &str, start: usize, end: usize, max_lines: usize) -> String {
    let end = end.min(text.len());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &text[line_start..start];
    let indent = if prefix.bytes().all(|c| c == b' ' || c == b'\t') { prefix.len() } else { 0 };
    let dedent = |l: &str| -> String {
        let ws = l.len() - l.trim_start_matches([' ', '\t']).len();
        l[ws.min(indent)..].to_string()
    };
    let lines: Vec<&str> = text[start..end].lines().collect();
    let mut out: Vec<String> = Vec::new();
    if lines.len() > max_lines && max_lines > 2 {
        out.push(lines[0].to_string());
        out.extend(lines[1..max_lines - 1].iter().map(|l| dedent(l)));
        out.push("  /* … */".to_string());
        out.push(dedent(lines[lines.len() - 1]));
    } else {
        out.extend(lines.iter().enumerate().map(|(n, l)| if n == 0 { l.to_string() } else { dedent(l) }));
    }
    out.join("\n").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str, scss: bool) -> Vec<String> {
        parse(src, 0, src.len(), scss).into_iter().map(|d| d.name).collect()
    }

    #[test]
    fn basic_selectors() {
        let src = r#"
/* .commented { } */
.btn, .btn-primary:hover > a.link { color: red; }
@media (min-width: 1.5em) { .card { padding: .5rem } }
[data-x=".nope"] .yes {}
@keyframes spin { 50.5% { opacity: .5 } }
.md\:flex { display: flex }
.\31 23 {}
"#;
        assert_eq!(names(src, false), ["btn", "btn-primary", "link", "card", "yes", "md:flex", "123"]);
    }

    #[test]
    fn rule_extent() {
        let src = ".a { color: red; }\n.b{}";
        let d = parse(src, 0, src.len(), false);
        assert_eq!(&src[d[0].rule_start..d[0].rule_end], ".a { color: red; }");
        assert_eq!(&src[d[0].start..d[0].end], ".a");
    }

    #[test]
    fn scss_nesting() {
        let src = ".card {\n  // .no { }\n  &__title { }\n  &:hover { &-x {} }\n  @media x { &--big {} }\n}";
        assert_eq!(names(src, true), ["card", "card__title", "card-x", "card--big"]);
    }

    #[test]
    fn tailwind_utility() {
        let src = "@layer components { .btn { @apply px-4; } }\n@utility tab-4 { tab-size: 4; }";
        assert_eq!(names(src, false), ["btn", "tab-4"]);
    }

    #[test]
    fn snippet_dedents() {
        let src = "@layer x {\n  .a {\n    color: red;\n  }\n}";
        let d = parse(src, 0, src.len(), false);
        assert_eq!(snippet(src, d[0].rule_start, d[0].rule_end, 30), ".a {\n  color: red;\n}");
    }
}
