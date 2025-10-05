use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tree_sitter::{Language, Parser, Tree};

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
    pub docs: DashMap<String, DocState>,
    pub lang: Language,
    pub debounce: Duration,
}

impl ServerState {
    pub fn insert_doc(&self, uri: String, text: Arc<str>) {
        self.docs.insert(uri, DocState::new(text));
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            docs: DashMap::new(),
            lang: tree_sitter_julia::LANGUAGE.into(),
            debounce: Duration::from_millis(120),
        }
    }
}
