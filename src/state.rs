use dashmap::DashMap;
use ignore::WalkBuilder;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
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
    extra_roots: RwLock<Vec<PathBuf>>,
    index_env: RwLock<bool>,
}

impl ServerState {
    pub fn insert_doc(&self, uri: String, text: Arc<str>) {
        self.docs.insert(uri, DocState::new(text));
    }

    pub fn set_root(&self, path: PathBuf) {
        *self.root.write() = Some(path);
    }

    pub fn root_path(&self) -> Option<PathBuf> {
        self.root.read().clone()
    }

    pub fn start_indexer(&self, root: PathBuf) {
        let docs = self.docs.clone();
        let mut roots = vec![root.clone()];
        roots.extend(discover_env_roots(&root));

        for r in roots {
            let docs_cloned = docs.clone();
            task::spawn_blocking(move || {
                index_workspace(&r, docs_cloned);
            });
        }
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            docs: Arc::new(DashMap::new()),
            lang: Arc::new(tree_sitter_julia::LANGUAGE.into()),
            debounce: Duration::from_millis(120),
            root: RwLock::new(None),
            extra_roots: RwLock::new(default_extra_roots()),
            index_env: RwLock::new(true),
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
            let path = dir_entry.path();
            if let Some(ext) = path.extension() {
                if ext == "jl" {
                    let is_depot = path.components().any(|c| {
                        if let std::path::Component::Normal(s) = c {
                            s == "packages" || s == "dev"
                        } else {
                            false
                        }
                    });
                    if is_depot {
                        let has_src = path.components().any(|c| {
                            if let std::path::Component::Normal(s) = c {
                                s == "src"
                            } else {
                                false
                            }
                        });
                        if !has_src {
                            continue;
                        }
                    }
                    if let Ok(text) = fs::read_to_string(path) {
                        if let Some(uri) = path_to_file_uri(path) {
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

fn default_extra_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(val) = std::env::var("PARSEC_EXTRA_INDEX_ROOTS") {
        for s in val.split(':') {
            if s.is_empty() {
                continue;
            }
            let expanded = shellexpand::tilde(s).to_string();
            out.push(PathBuf::from(expanded));
        }
    }
    let depots: Vec<PathBuf> = std::env::var("JULIA_DEPOT_PATH")
        .ok()
        .map(|s| {
            s.split(':')
                .map(|p| shellexpand::tilde(p).to_string())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_else(|| vec![dirs::home_dir().unwrap_or_default().join(".julia")]);
    for d in depots {
        out.push(d.join("packages"));
        out.push(d.join("dev"));
    }
    out
}

fn discover_env_roots(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let project_toml = root.join("Project.toml");
    if !project_toml.exists() {
        return out;
    }
    let depots: Vec<PathBuf> = std::env::var("JULIA_DEPOT_PATH")
        .ok()
        .map(|s| {
            s.split(':')
                .map(|p| shellexpand::tilde(p).to_string())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_else(|| vec![dirs::home_dir().unwrap_or_default().join(".julia")]);
    let deps = read_project_deps(&project_toml);
    if deps.is_empty() {
        return out;
    }
    tracing::info!("Found {} deps in Project.toml: {:?}", deps.len(), deps);
    for d in depots {
        let pkgs = d.join("packages");
        let dev = d.join("dev");
        for name in &deps {
            tracing::info!("Indexing paths for dep '{}': {:?} and {:?}", name, pkgs.join(name), dev.join(name));
            out.push(pkgs.join(name));
            out.push(dev.join(name));
        }
    }
    out
}

fn read_project_deps(file: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(src) = std::fs::read_to_string(file) {
        if let Ok(doc) = toml::from_str::<toml::Value>(&src) {
            if let Some(deps) = doc.get("deps").and_then(|v| v.as_table()) {
                for (name, _) in deps {
                    out.insert(name.clone());
                }
            }
        }
    }
    out
}
