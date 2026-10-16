mod config;
mod css;
mod index;
mod line_index;
mod uri;
mod usage;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::*;
use serde::de::DeserializeOwned;
use serde_json::Value;

use config::Config;
use index::{FileData, Index};
use line_index::{Encoding, LineIndex};

const MAX_HOVER_DEFS: usize = 5;

struct Doc {
    path: PathBuf,
    text: String,
    li: LineIndex,
}

enum Internal {
    Indexed(HashMap<PathBuf, FileData>, Duration),
}

struct Server {
    conn: Connection,
    enc: Encoding,
    roots: Vec<PathBuf>,
    cfg: Arc<Config>,
    index: Index,
    docs: HashMap<String, Doc>,
    gitignores: Vec<ignore::gitignore::Gitignore>,
    tx: Sender<Internal>,
    rx: Receiver<Internal>,
    indexing: bool,
    reindex_pending: bool,
}

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("css-class-lsp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let (conn, io) = Connection::stdio();
    let (id, params) = conn.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;

    let utf8 = params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref())
        .is_some_and(|e| e.contains(&PositionEncodingKind::UTF8));
    let enc = if utf8 { Encoding::Utf8 } else { Encoding::Utf16 };

    let caps = ServerCapabilities {
        position_encoding: Some(enc.kind()),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(["\"", "'", "`", " ", "-"].map(String::from).to_vec()),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    let init = InitializeResult {
        capabilities: caps,
        server_info: Some(ServerInfo {
            name: "css-class-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    conn.initialize_finish(id, serde_json::to_value(init)?)?;

    #[allow(deprecated)]
    let mut roots: Vec<PathBuf> = match &params.workspace_folders {
        Some(f) if !f.is_empty() => f.iter().filter_map(|f| uri::to_path(&f.uri)).collect(),
        _ => params.root_uri.as_ref().and_then(uri::to_path).into_iter().collect(),
    };
    roots.dedup();

    let watch = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files.as_ref())
        .and_then(|d| d.dynamic_registration)
        .unwrap_or(false);

    let cfg = Config::from_value(params.initialization_options);
    let (tx, rx) = crossbeam_channel::unbounded();
    let gitignores = roots
        .iter()
        .map(|r| ignore::gitignore::Gitignore::new(r.join(".gitignore")).0)
        .collect();
    let mut server = Server {
        conn,
        enc,
        roots,
        cfg: Arc::new(cfg),
        index: Index::default(),
        docs: HashMap::new(),
        gitignores,
        tx,
        rx,
        indexing: false,
        reindex_pending: false,
    };
    for e in server.cfg.errors.clone() {
        server.show(MessageType::ERROR, format!("css-class-lsp: {e}"));
    }
    if watch {
        server.register_watchers();
    }
    server.start_index();
    server.run()?;
    drop(server);
    io.join()?;
    Ok(())
}

fn parse<P: DeserializeOwned>(v: Value) -> Result<P> {
    Ok(serde_json::from_value(v)?)
}

impl Server {
    fn run(&mut self) -> Result<()> {
        let lsp_rx = self.conn.receiver.clone();
        let rx = self.rx.clone();
        loop {
            crossbeam_channel::select! {
                recv(lsp_rx) -> msg => {
                    let Ok(msg) = msg else { return Ok(()) };
                    match msg {
                        Message::Request(req) => {
                            if self.conn.handle_shutdown(&req)? {
                                return Ok(());
                            }
                            self.on_request(req);
                        }
                        Message::Notification(n) => {
                            if n.method == "exit" {
                                return Ok(());
                            }
                            if let Err(e) = self.on_notification(n) {
                                self.log(format!("notification error: {e:#}"));
                            }
                        }
                        Message::Response(_) => {}
                    }
                }
                recv(rx) -> msg => {
                    if let Ok(Internal::Indexed(files, took)) = msg {
                        self.on_indexed(files, took);
                    }
                }
            }
        }
    }

    fn send(&self, msg: impl Into<Message>) {
        let _ = self.conn.sender.send(msg.into());
    }

    fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.send(Notification::new(N::METHOD.to_string(), params));
    }

    fn log(&self, message: String) {
        self.notify::<notification::LogMessage>(LogMessageParams { typ: MessageType::LOG, message });
    }

    fn show(&self, typ: MessageType, message: String) {
        self.notify::<notification::ShowMessage>(ShowMessageParams { typ, message });
    }

    fn register_watchers(&self) {
        let glob = format!("**/*.{{{}}}", self.cfg.all_extensions().join(","));
        let opts = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher { glob_pattern: GlobPattern::String(glob), kind: None }],
        };
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "css-class-lsp/watch".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: serde_json::to_value(opts).ok(),
            }],
        };
        self.send(Request::new(
            RequestId::from("css-class-lsp/register".to_string()),
            "client/registerCapability".into(),
            params,
        ));
    }

    fn start_index(&mut self) {
        if self.indexing {
            self.reindex_pending = true;
            return;
        }
        self.indexing = true;
        let (roots, cfg, tx) = (self.roots.clone(), self.cfg.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let t = Instant::now();
            let files = index::scan(&roots, &cfg);
            let _ = tx.send(Internal::Indexed(files, t.elapsed()));
        });
    }

    fn on_indexed(&mut self, files: HashMap<PathBuf, FileData>, took: Duration) {
        self.indexing = false;
        let (n_files, n_css) = (files.len(), files.values().filter(|f| f.is_css).count());
        self.index.replace_all(files);
        // Open buffers may have unsaved changes; they win over disk contents.
        let open: Vec<_> = self.docs.values().map(|d| (d.path.clone(), index::analyze(&d.path, &d.text, &self.cfg))).collect();
        for (p, d) in open {
            self.index.set(p, d);
        }
        let n_classes = self.index.summary().len();
        self.log(format!(
            "indexed {n_classes} classes from {n_css} stylesheets ({n_files} files) in {:.0?}",
            took
        ));
        if std::mem::take(&mut self.reindex_pending) {
            self.start_index();
        }
    }

    fn update_doc(&mut self, uri: &Uri, text: String) {
        let Some(path) = uri::to_path(uri) else { return };
        let data = index::analyze(&path, &text, &self.cfg);
        self.index.set(path.clone(), data);
        let li = LineIndex::new(&text);
        self.docs.insert(uri.as_str().to_string(), Doc { path, text, li });
    }

    fn on_notification(&mut self, n: Notification) -> Result<()> {
        match n.method.as_str() {
            "textDocument/didOpen" => {
                let p: DidOpenTextDocumentParams = parse(n.params)?;
                self.update_doc(&p.text_document.uri, p.text_document.text);
            }
            "textDocument/didChange" => {
                let p: DidChangeTextDocumentParams = parse(n.params)?;
                if let Some(c) = p.content_changes.into_iter().last() {
                    self.update_doc(&p.text_document.uri, c.text);
                }
            }
            "textDocument/didClose" => {
                let p: DidCloseTextDocumentParams = parse(n.params)?;
                if let Some(doc) = self.docs.remove(p.text_document.uri.as_str()) {
                    // Fall back to what is on disk (changes may have been discarded).
                    let data = self.should_index(&doc.path).then(|| index::read_and_analyze(&doc.path, &self.cfg)).flatten();
                    self.index.set(doc.path, data);
                }
            }
            "workspace/didChangeWatchedFiles" => {
                let p: DidChangeWatchedFilesParams = parse(n.params)?;
                for ev in p.changes {
                    let Some(path) = uri::to_path(&ev.uri) else { continue };
                    if self.docs.values().any(|d| d.path == path) {
                        continue;
                    }
                    let data = if ev.typ == FileChangeType::DELETED || !self.should_index(&path) {
                        None
                    } else {
                        index::read_and_analyze(&path, &self.cfg)
                    };
                    self.index.set(path, data);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Whether a file outside of a full scan belongs in the index.
    fn should_index(&self, path: &Path) -> bool {
        if !self.cfg.is_relevant(path) || self.cfg.is_excluded(path, &self.roots) {
            return false;
        }
        if self.index.files.contains_key(path) {
            return true;
        }
        let in_root = self.roots.iter().any(|r| path.starts_with(r));
        let ignored = self.cfg.respect_gitignore
            && self.gitignores.iter().any(|g| path.starts_with(g.path()) && g.matched_path_or_any_parents(path, false).is_ignore());
        in_root && !ignored
    }

    fn on_request(&mut self, req: Request) {
        let id = req.id.clone();
        let result = match req.method.as_str() {
            "textDocument/completion" => parse(req.params).and_then(|p| self.completion(p)),
            "completionItem/resolve" => parse(req.params).and_then(|p| self.resolve(p)),
            "textDocument/hover" => parse(req.params).and_then(|p| self.hover(p)),
            "textDocument/definition" => parse(req.params).and_then(|p| self.definition(p)),
            "textDocument/references" => parse(req.params).and_then(|p| self.references(p)),
            "workspace/symbol" => parse(req.params).and_then(|p| self.symbols(p)),
            "cssClassLsp/reindex" => {
                self.start_index();
                Ok(Value::Null)
            }
            "cssClassLsp/stats" => Ok(self.stats()),
            m => {
                let msg = format!("unhandled method {m}");
                self.send(Response::new_err(id, ErrorCode::MethodNotFound as i32, msg));
                return;
            }
        };
        let resp = match result {
            Ok(v) => Response::new_ok(id, v),
            Err(e) => Response::new_err(id, ErrorCode::InternalError as i32, format!("{e:#}")),
        };
        self.send(resp);
    }

    fn rel<'a>(&self, path: &'a Path) -> std::borrow::Cow<'a, str> {
        self.roots
            .iter()
            .filter_map(|r| path.strip_prefix(r).ok())
            .min_by_key(|p| p.as_os_str().len())
            .unwrap_or(path)
            .to_string_lossy()
    }

    /// The class (and its span) under the cursor, from the indexed data.
    fn class_at(&self, uri: &Uri, pos: Position) -> Option<(String, Range)> {
        let fd = self.index.files.get(&uri::to_path(uri)?)?;
        let e = self.enc;
        fd.defs
            .iter()
            .find(|d| d.span.contains(pos, e))
            .map(|d| (d.name.clone(), d.span.range(e)))
            .or_else(|| fd.usages.iter().find(|u| u.span.contains(pos, e)).map(|u| (u.name.clone(), u.span.range(e))))
    }

    fn class_markdown(&mut self, name: &str) -> Option<String> {
        let s = self.index.summary().get(name)?;
        let (defs, refs) = (s.defs.clone(), s.refs);
        if defs.is_empty() {
            return None;
        }
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let mut md = format!(
            "**`.{name}`** — {} · {}\n",
            plural(defs.len(), "definition"),
            plural(refs, "reference")
        );
        for (path, i) in defs.iter().take(MAX_HOVER_DEFS) {
            let Some(d) = self.index.def(path, *i) else { continue };
            let _ = write!(md, "\n`{}:{}`\n```{}\n{}\n```\n", self.rel(path), d.span.line + 1, d.lang, d.rule);
        }
        if defs.len() > MAX_HOVER_DEFS {
            let _ = write!(md, "\n_…and {} more_\n", defs.len() - MAX_HOVER_DEFS);
        }
        Some(md)
    }

    fn completion(&mut self, p: CompletionParams) -> Result<Value> {
        let tdp = p.text_document_position;
        let Some(doc) = self.docs.get(tdp.text_document.uri.as_str()) else { return Ok(Value::Null) };
        let offset = doc.li.offset(&doc.text, tdp.position, self.enc);
        let is_css = self.cfg.css_ext.contains(&doc.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase());
        let regions = if is_css {
            usage::regions_for(&self.cfg.apply, &doc.text)
        } else {
            usage::class_regions(&doc.text, &self.cfg.patterns)
        };
        let Some(start) = usage::completion_start(&doc.text, offset, &regions, is_css) else {
            return Ok(Value::Null);
        };
        let range = Range::new(doc.li.position(&doc.text, start, self.enc), tdp.position);

        let entries: Vec<(String, PathBuf, usize, usize, usize)> = self
            .index
            .summary()
            .iter()
            .filter_map(|(name, s)| s.defs.first().map(|(p, i)| (name.clone(), p.clone(), *i, s.defs.len(), s.refs)))
            .collect();
        let items: Vec<CompletionItem> = entries
            .into_iter()
            .map(|(name, path, i, n_defs, refs)| {
                let line = self.index.def(&path, i).map_or(0, |d| d.span.line + 1);
                let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let more = if n_defs > 1 { format!(" +{}", n_defs - 1) } else { String::new() };
                CompletionItem {
                    label: name.clone(),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: None,
                        description: Some(format!("{file}{more}")),
                    }),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some(format!("{}:{line} · {refs} refs", self.rel(&path))),
                    filter_text: Some(name.clone()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, name.clone()))),
                    data: Some(Value::String(name)),
                    ..Default::default()
                }
            })
            .collect();
        Ok(serde_json::to_value(CompletionResponse::List(CompletionList { is_incomplete: false, items }))?)
    }

    fn resolve(&mut self, mut item: CompletionItem) -> Result<Value> {
        let name = item.data.as_ref().and_then(|d| d.as_str()).unwrap_or(&item.label).to_string();
        if let Some(md) = self.class_markdown(&name) {
            item.documentation = Some(Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value: md }));
        }
        Ok(serde_json::to_value(item)?)
    }

    fn hover(&mut self, p: HoverParams) -> Result<Value> {
        let tdp = p.text_document_position_params;
        let Some((name, range)) = self.class_at(&tdp.text_document.uri, tdp.position) else { return Ok(Value::Null) };
        let Some(md) = self.class_markdown(&name) else { return Ok(Value::Null) };
        Ok(serde_json::to_value(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: md }),
            range: Some(range),
        })?)
    }

    fn def_locations(&mut self, name: &str) -> Vec<Location> {
        let defs = self.index.summary().get(name).map(|s| s.defs.clone()).unwrap_or_default();
        defs.iter()
            .filter_map(|(p, i)| self.index.def(p, *i).map(|d| Location::new(uri::from_path(p), d.span.range(self.enc))))
            .collect()
    }

    fn definition(&mut self, p: GotoDefinitionParams) -> Result<Value> {
        let tdp = p.text_document_position_params;
        let Some((name, _)) = self.class_at(&tdp.text_document.uri, tdp.position) else { return Ok(Value::Null) };
        let locs = self.def_locations(&name);
        if locs.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::to_value(GotoDefinitionResponse::Array(locs))?)
    }

    fn references(&mut self, p: ReferenceParams) -> Result<Value> {
        let tdp = p.text_document_position;
        let Some((name, _)) = self.class_at(&tdp.text_document.uri, tdp.position) else { return Ok(Value::Null) };
        let mut locs = if p.context.include_declaration { self.def_locations(&name) } else { Vec::new() };
        let mut usages: Vec<_> = self.index.usages_of(&name).map(|(p, u)| (p.clone(), u.span)).collect();
        usages.sort_by(|a, b| (&a.0, a.1.line, a.1.start8).cmp(&(&b.0, b.1.line, b.1.start8)));
        locs.extend(usages.into_iter().map(|(p, s)| Location::new(uri::from_path(&p), s.range(self.enc))));
        Ok(serde_json::to_value(locs)?)
    }

    fn symbols(&mut self, p: WorkspaceSymbolParams) -> Result<Value> {
        let q = p.query.to_lowercase();
        let mut out = Vec::new();
        let mut names: Vec<(String, Vec<(PathBuf, usize)>)> = self
            .index
            .summary()
            .iter()
            .filter(|(n, s)| !s.defs.is_empty() && n.to_lowercase().contains(&q))
            .map(|(n, s)| (n.clone(), s.defs.clone()))
            .collect();
        names.sort();
        for (name, defs) in names {
            for (path, i) in defs {
                let Some(d) = self.index.def(&path, i) else { continue };
                #[allow(deprecated)]
                out.push(SymbolInformation {
                    name: format!(".{name}"),
                    kind: SymbolKind::CLASS,
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri::from_path(&path), d.span.range(self.enc)),
                    container_name: Some(self.rel(&path).into_owned()),
                });
            }
        }
        Ok(serde_json::to_value(out)?)
    }

    fn stats(&mut self) -> Value {
        let files = self.index.files.len();
        let stylesheets: Vec<String> = {
            let mut v: Vec<_> = self.index.files.iter().filter(|(_, f)| f.is_css || !f.defs.is_empty()).map(|(p, _)| self.rel(p).into_owned()).collect();
            v.sort();
            v
        };
        let classes = self.index.summary().len();
        serde_json::json!({
            "roots": self.roots,
            "files": files,
            "classes": classes,
            "stylesheets": stylesheets,
            "indexing": self.indexing,
        })
    }
}
