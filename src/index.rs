use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use rayon::prelude::*;
use regex::Regex;

use crate::config::Config;
use crate::css;
use crate::line_index::{LineIndex, Span};
use crate::usage;

const SNIPPET_LINES: usize = 25;

#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: String,
    pub span: Span,
    pub rule: String,
    pub lang: &'static str,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Default, Clone)]
pub struct FileData {
    pub defs: Vec<ClassDef>,
    pub usages: Vec<Usage>,
    pub is_css: bool,
}

#[derive(Default)]
pub struct Summary {
    /// (file, index into FileData::defs)
    pub defs: Vec<(PathBuf, usize)>,
    pub refs: usize,
}

#[derive(Default)]
pub struct Index {
    pub files: HashMap<PathBuf, FileData>,
    summary: Option<HashMap<String, Summary>>,
}

impl Index {
    pub fn set(&mut self, path: PathBuf, data: Option<FileData>) {
        match data {
            Some(d) => self.files.insert(path, d),
            None => self.files.remove(&path),
        };
        self.summary = None;
    }

    pub fn replace_all(&mut self, files: HashMap<PathBuf, FileData>) {
        self.files = files;
        self.summary = None;
    }

    pub fn summary(&mut self) -> &HashMap<String, Summary> {
        self.summary.get_or_insert_with(|| {
            let mut m: HashMap<String, Summary> = HashMap::new();
            for (path, fd) in &self.files {
                for (i, d) in fd.defs.iter().enumerate() {
                    m.entry(d.name.clone()).or_default().defs.push((path.clone(), i));
                }
            }
            for fd in self.files.values() {
                for u in &fd.usages {
                    if let Some(s) = m.get_mut(&u.name) {
                        s.refs += 1;
                    }
                }
            }
            for s in m.values_mut() {
                s.defs.sort();
            }
            m
        })
    }

    pub fn def(&self, path: &Path, i: usize) -> Option<&ClassDef> {
        self.files.get(path)?.defs.get(i)
    }

    pub fn usages_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (&'a PathBuf, &'a Usage)> + 'a {
        self.files
            .iter()
            .flat_map(|(p, fd)| fd.usages.iter().map(move |u| (p, u)))
            .filter(move |(_, u)| u.name == name)
    }
}

fn ext_of(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

fn is_generated(path: &Path, text: &str) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let head = text.trim_start();
    name.ends_with(".min.css") || head.starts_with("/*! tailwindcss v") || head.starts_with("/* ! tailwindcss v")
}

/// Extract class definitions and usages from one file. `None` means the file
/// is not something we index.
pub fn analyze(path: &Path, text: &str, cfg: &Config) -> Option<FileData> {
    static STYLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?is)<style\b([^>]*)>(.*?)</style>"#).unwrap());

    let ext = ext_of(path)?;
    let li = LineIndex::new(text);
    let mut fd = FileData::default();

    let add_defs = |fd: &mut FileData, start: usize, end: usize, scss: bool| {
        for d in css::parse(text, start, end, scss) {
            fd.defs.push(ClassDef {
                span: Span::new(text, &li, d.start, d.end),
                rule: css::snippet(text, d.rule_start, d.rule_end, SNIPPET_LINES),
                lang: if scss { "scss" } else { "css" },
                name: d.name,
            });
        }
    };
    let add_usages = |fd: &mut FileData, regions: Vec<(usize, usize)>| {
        for r in regions {
            for (s, e) in usage::tokens(text, r) {
                fd.usages.push(Usage { name: text[s..e].to_string(), span: Span::new(text, &li, s, e) });
            }
        }
    };

    if cfg.css_ext.contains(&ext) {
        if cfg.skip_generated && is_generated(path, text) {
            return None;
        }
        fd.is_css = true;
        add_defs(&mut fd, 0, text.len(), matches!(ext.as_str(), "scss" | "less"));
        add_usages(&mut fd, usage::regions_for(&cfg.apply, text));
    } else if cfg.source_ext.contains(&ext) {
        add_usages(&mut fd, usage::class_regions(text, &cfg.patterns));
        if matches!(ext.as_str(), "html" | "htm" | "vue" | "svelte" | "astro") {
            for c in STYLE.captures_iter(text) {
                let (attrs, body) = (c.get(1).unwrap().as_str(), c.get(2).unwrap());
                let scss = attrs.contains("scss") || attrs.contains("less");
                add_defs(&mut fd, body.start(), body.end(), scss);
                add_usages(&mut fd, usage::regions_for(&cfg.apply, body.as_str())
                    .into_iter()
                    .map(|(s, e)| (s + body.start(), e + body.start()))
                    .collect());
            }
        }
    } else {
        return None;
    }
    Some(fd)
}

pub fn read_and_analyze(path: &Path, cfg: &Config) -> Option<FileData> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cfg.max_file_size {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    analyze(path, &text, cfg)
}

pub fn scan(roots: &[PathBuf], cfg: &Config) -> HashMap<PathBuf, FileData> {
    let mut paths = Vec::new();
    for root in roots {
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(cfg.respect_gitignore)
            .git_global(cfg.respect_gitignore)
            .git_exclude(cfg.respect_gitignore)
            .ignore(cfg.respect_gitignore)
            .require_git(false)
            .filter_entry({
                let roots = roots.to_vec();
                let cfg_exclude = cfg.exclude.clone();
                move |e| {
                    let p = e.path();
                    let probe = if e.file_type().is_some_and(|t| t.is_dir()) { p.join("_") } else { p.to_path_buf() };
                    !(cfg_exclude.is_match(&probe)
                        || roots.iter().filter_map(|r| probe.strip_prefix(r).ok()).any(|r| cfg_exclude.is_match(r)))
                }
            })
            .build();
        for entry in walker.flatten() {
            if entry.file_type().is_some_and(|t| t.is_file()) && cfg.is_relevant(entry.path()) {
                paths.push(entry.into_path());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_par_iter()
        .filter_map(|p| read_and_analyze(&p, cfg).map(|d| (p, d)))
        .collect()
}
