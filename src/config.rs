use std::collections::HashSet;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::Deserialize;

/// Regexes that locate strings containing class names. Every capture group
/// is treated as a whitespace separated list of classes.
const DEFAULT_PATTERNS: &[&str] = &[
    // HTML / JSX / Vue / Svelte / RSX: class="..." className="..." class: "..."
    r#"\b(?:class|className)\s*[:=]\s*"([^"]*)""#,
    r#"\b(?:class|className)\s*[:=]\s*'([^']*)'"#,
    // JSX: className={"..."} / className={`...`}
    r#"\b(?:class|className)\s*=\s*\{\s*["'`]([^"'`]*)["'`]"#,
    // Dioxus: class: if cond { "a" } else { "b" }
    r#"\bclass\s*:\s*if\b[^{]*\{\s*"([^"]*)"\s*\}(?:\s*else\s*\{\s*"([^"]*)"\s*\})?"#,
    // Dioxus: class: format!("...")
    r#"\bclass\s*:\s*format!\(\s*"([^"]*)""#,
];

/// [container, inner] pairs, same semantics as Tailwind's classRegex arrays.
const DEFAULT_NESTED: &[(&str, &str)] = &[(
    r#"\b(?:clsx|cn|cx|classnames|classNames|twMerge|twJoin)\(([^)]*)\)"#,
    r#"["'`]([^"'`]*)["'`]"#,
)];

const DEFAULT_CSS_EXT: &[&str] = &["css", "scss", "less", "pcss", "postcss"];
const DEFAULT_SOURCE_EXT: &[&str] = &[
    "rs", "html", "htm", "jsx", "tsx", "js", "ts", "mjs", "vue", "svelte", "astro", "heex", "erb",
    "hbs", "njk", "twig", "php", "templ",
];
const DEFAULT_EXCLUDE: &[&str] = &["**/node_modules/**", "**/target/**", "**/.git/**"];

#[derive(Deserialize)]
#[serde(untagged)]
enum RawPattern {
    Single(String),
    Nested(Vec<String>),
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    css_extensions: Option<Vec<String>>,
    source_extensions: Option<Vec<String>>,
    class_regex: Vec<RawPattern>,
    disable_default_patterns: bool,
    exclude: Vec<String>,
    max_file_size: Option<u64>,
    respect_gitignore: Option<bool>,
    skip_generated: Option<bool>,
}

pub enum Pattern {
    Single(Regex),
    Nested(Regex, Regex),
}

pub struct Config {
    pub css_ext: HashSet<String>,
    pub source_ext: HashSet<String>,
    pub patterns: Vec<Pattern>,
    pub apply: Regex,
    pub exclude: GlobSet,
    pub max_file_size: u64,
    pub respect_gitignore: bool,
    pub skip_generated: bool,
    /// Problems found while reading the configuration, reported to the user.
    pub errors: Vec<String>,
}

impl Config {
    pub fn from_value(value: Option<serde_json::Value>) -> Config {
        let mut errors = Vec::new();
        let raw: RawConfig = match value {
            Some(v) if !v.is_null() => serde_json::from_value(v).unwrap_or_else(|e| {
                errors.push(format!("invalid init_options: {e}"));
                RawConfig::default()
            }),
            _ => RawConfig::default(),
        };

        let exts = |v: Option<Vec<String>>, d: &[&str]| -> HashSet<String> {
            v.unwrap_or_else(|| d.iter().map(|s| s.to_string()).collect())
                .into_iter()
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
                .collect()
        };

        let mut regex_errors = Vec::new();
        let mut compile = |src: &str| match Regex::new(src) {
            Ok(r) => Some(r),
            Err(e) => {
                regex_errors.push(format!("invalid class_regex {src:?}: {e}"));
                None
            }
        };

        let mut patterns = Vec::new();
        for p in &raw.class_regex {
            match p {
                RawPattern::Single(s) => patterns.extend(compile(s).map(Pattern::Single)),
                RawPattern::Nested(v) if v.len() == 2 => {
                    if let (Some(a), Some(b)) = (compile(&v[0]), compile(&v[1])) {
                        patterns.push(Pattern::Nested(a, b));
                    }
                }
                RawPattern::Nested(v) => {
                    errors.push(format!("class_regex arrays need exactly 2 entries: {v:?}"))
                }
            }
        }
        errors.extend(regex_errors);
        if !raw.disable_default_patterns {
            patterns.extend(DEFAULT_PATTERNS.iter().map(|s| Pattern::Single(Regex::new(s).unwrap())));
            patterns.extend(DEFAULT_NESTED.iter().map(|(a, b)| {
                Pattern::Nested(Regex::new(a).unwrap(), Regex::new(b).unwrap())
            }));
        }

        let mut gb = GlobSetBuilder::new();
        for g in DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).chain(raw.exclude) {
            match Glob::new(&g) {
                Ok(g) => {
                    gb.add(g);
                }
                Err(e) => errors.push(format!("invalid exclude glob {g:?}: {e}")),
            }
        }

        Config {
            css_ext: exts(raw.css_extensions, DEFAULT_CSS_EXT),
            source_ext: exts(raw.source_extensions, DEFAULT_SOURCE_EXT),
            patterns,
            apply: Regex::new(r"@apply\s+([^;{}]*)").unwrap(),
            exclude: gb.build().unwrap_or_else(|_| GlobSet::empty()),
            max_file_size: raw.max_file_size.unwrap_or(2 * 1024 * 1024),
            respect_gitignore: raw.respect_gitignore.unwrap_or(true),
            skip_generated: raw.skip_generated.unwrap_or(true),
            errors,
        }
    }

    pub fn is_excluded(&self, path: &Path, roots: &[std::path::PathBuf]) -> bool {
        self.exclude.is_match(path)
            || roots
                .iter()
                .filter_map(|r| path.strip_prefix(r).ok())
                .any(|rel| self.exclude.is_match(rel))
    }

    pub fn is_relevant(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                self.css_ext.contains(&e) || self.source_ext.contains(&e)
            })
            .unwrap_or(false)
    }

    pub fn all_extensions(&self) -> Vec<String> {
        let mut v: Vec<_> = self.css_ext.iter().chain(&self.source_ext).cloned().collect();
        v.sort();
        v
    }
}
