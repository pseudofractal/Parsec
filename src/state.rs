use dashmap::DashMap;
use ignore::WalkBuilder;
use parking_lot::RwLock;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task;
use tree_sitter::{Language, Parser, Tree};
use url::Url;

pub struct DocState {
    text: Arc<str>,
    tree: RwLock<Option<Tree>>,
    last_edit: RwLock<Instant>,
    last_parse: RwLock<Instant>,
}

impl DocState {
    pub fn new(text: Arc<str>) -> Self {
        let now = Instant::now();
        Self {
            text,
            tree: RwLock::new(None),
            last_edit: RwLock::new(now),
            last_parse: RwLock::new(Instant::now() - Duration::from_secs(1)),
        }
    }

    pub fn update_text(&mut self, text: Arc<str>) {
        self.text = text;
        *self.last_edit.write() = Instant::now();
    }

    pub fn text(&self) -> String {
        self.text.to_string()
    }

    pub fn parse_with_debounce(&self, lang: &Language, min_delay: Duration) {
        let edited_at = *self.last_edit.read();
        let parsed_at = *self.last_parse.read();
        if parsed_at >= edited_at && self.tree.read().is_some() {
            return;
        }
        if edited_at.elapsed() < min_delay && self.tree.read().is_some() {
            return;
        }
        let mut parser = Parser::new();
        parser.set_language(lang).unwrap();
        let tree = parser.parse(&*self.text, None);
        *self.tree.write() = tree;
        *self.last_parse.write() = Instant::now();
    }

    pub fn current_tree(&self) -> Option<Tree> {
        self.tree.read().clone()
    }
}

pub struct ServerState {
    pub docs: Arc<DashMap<String, DocState>>,
    pub lang: Arc<Language>,
    pub debounce: Duration,
    root: RwLock<Option<PathBuf>>,
}

impl ServerState {
    pub fn insert_doc(&self, uri: String, text: Arc<str>) {
        self.docs.insert(uri, DocState::new(text));
    }

    pub fn set_root(&self, path: PathBuf) {
        *self.root.write() = Some(path);
    }

    pub fn start_indexer(&self, root: PathBuf) {
        let docs = self.docs.clone();
        task::spawn_blocking(move || {
            index_workspace(&root, docs);
        });
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            docs: Arc::new(DashMap::new()),
            lang: Arc::new(tree_sitter_julia::LANGUAGE.into()),
            debounce: Duration::from_millis(120),
            root: RwLock::new(None),
        }
    }
}

fn index_workspace(root: &Path, docs: Arc<DashMap<String, DocState>>) {
    let mut types = ignore::types::TypesBuilder::new();
    types.add_defaults();
    types.select("jl");
    types.add("jl", "*.jl").unwrap();
    let types = types.build().unwrap();

    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .types(types)
        .build();

    for entry in walker {
        if let Ok(dir_entry) = entry {
            if let Some(ext) = dir_entry.path().extension() {
                if ext == "jl" {
                    if let Ok(text) = fs::read_to_string(dir_entry.path()) {
                        if let Some(uri) = path_to_file_uri(dir_entry.path()) {
                            docs.insert(uri, DocState::new(text.into()));
                        }
                    }
                }
            }
        }
    }
}

fn path_to_file_uri(path: &Path) -> Option<String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let url = Url::from_file_path(abs).ok()?;
    Some(url.to_string())
}
