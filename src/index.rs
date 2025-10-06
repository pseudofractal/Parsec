use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tower_lsp::lsp_types::{Location, Range, SymbolInformation, SymbolKind, Url};

#[derive(Clone)]
pub struct SymbolEntry {
    pub name: Arc<str>,
    pub name_lowercase: Arc<str>,
    pub uri: Url,
    pub path: PathBuf,
    pub range: Range,
    pub kind: SymbolKind,
}

pub struct SymbolIndex {
    by_doc: DashMap<String, Arc<[SymbolEntry]>>,
}

impl Default for SymbolIndex {
    fn default() -> Self {
        Self {
            by_doc: DashMap::new(),
        }
    }
}

impl SymbolIndex {
    pub fn upsert_doc(&self, doc_uri: &Url, symbols: Vec<SymbolInformation>) {
        let mut out: Vec<SymbolEntry> = Vec::with_capacity(symbols.len());
        let path = doc_uri.to_file_path().ok().unwrap_or_default();
        for symbol in symbols {
            let name: Arc<str> = Arc::from(symbol.name);
            let name_lowercase: Arc<str> = Arc::from(name.to_ascii_lowercase());
            out.push(SymbolEntry {
                name,
                name_lowercase,
                uri: symbol.location.uri,
                path: path.clone(),
                range: symbol.location.range,
                kind: symbol.kind,
            });
        }
        self.by_doc.insert(doc_uri.to_string(), out.into());
    }

    pub fn search_fuzzy(
        &self,
        query: &str,
        root: Option<&std::path::Path>,
        limit: usize,
    ) -> Vec<tower_lsp::lsp_types::SymbolInformation> {
        if limit == 0 {
            return Vec::new();
        }

        let blocks: Vec<std::sync::Arc<[SymbolEntry]>> = self
            .by_doc
            .iter()
            .map(|kv| std::sync::Arc::clone(kv.value()))
            .collect();

        let q = query.trim();
        if q.is_empty() {
            let mut out = Vec::with_capacity(limit.min(256));
            'outer: for blk in &blocks {
                for e in blk.iter() {
                    if root.is_none_or(|r| e.path.starts_with(r)) {
                        out.push(to_lsp(e));
                        if out.len() >= limit {
                            break 'outer;
                        }
                    }
                }
            }
            return out;
        }

        let qlc = q.to_ascii_lowercase();

        type Key = (i64, i64, i64, usize, usize);
        let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<Key>> =
            std::collections::BinaryHeap::new();
        let mut idx_counter: usize = 0;

        for (bi, blk) in blocks.iter().enumerate() {
            for (ei, e) in blk.iter().enumerate() {
                idx_counter = idx_counter.wrapping_add(1);
                if !root.is_none_or(|r| e.path.starts_with(r)) {
                    continue;
                }
                if let Some(score) = fuzzy_score(&qlc, &e.name, &e.name_lowercase) {
                    let key: Key = (score, -(e.name.len() as i64), -(idx_counter as i64), bi, ei);
                    heap.push(std::cmp::Reverse(key));
                    if heap.len() > limit {
                        let _ = heap.pop();
                    }
                }
            }
        }

        let mut keys: Vec<Key> = heap.into_iter().map(|std::cmp::Reverse(k)| k).collect();
        keys.sort_unstable_by(|a, b| b.cmp(a)); // score desc, then shorter names, then insertion

        let mut out = Vec::with_capacity(keys.len());
        for (_sc, _neg_len, _neg_idx, bi, ei) in keys {
            let e = &blocks[bi][ei];
            out.push(to_lsp(e));
        }
        out
    }
}

fn to_lsp(e: &SymbolEntry) -> SymbolInformation {
    #[allow(deprecated)]
    SymbolInformation {
        name: e.name.to_string(),
        kind: e.kind,
        tags: None,
        deprecated: None,
        location: Location {
            uri: e.uri.clone(),
            range: e.range,
        },
        container_name: None,
    }
}

// GPT Magic
fn fuzzy_score(q_lc: &str, name: &str, name_lc: &str) -> Option<i64> {
    if q_lc.is_empty() {
        return Some(0);
    }
    let qb = q_lc.as_bytes();
    let nb = name.as_bytes();
    let nblc = name_lc.as_bytes();

    let mut qi = 0usize;
    let mut score: i64 = 0;
    let mut last_match: Option<usize> = None;

    for i in 0..nblc.len() {
        if qi >= qb.len() {
            break;
        }
        if nblc[i] == qb[qi] {
            let mut s: i64 = 10;

            let prev = if i == 0 { b' ' } else { nb[i - 1] };
            if is_boundary(prev) {
                s += 15;
            }

            if i > 0 && nb[i].is_ascii_uppercase() && nb[i - 1].is_ascii_lowercase() {
                s += 12;
            }
            if let Some(last) = last_match {
                if i == last + 1 {
                    s += 8;
                } else {
                    let gap = (i - last - 1) as i64;
                    s -= 2 * gap.min(8); // cap penalty
                }
            } else {
                if i < 3 {
                    s += 5;
                }
            }

            score += s;
            last_match = Some(i);
            qi += 1;
        }
    }
    if qi == qb.len() { Some(score) } else { None }
}

fn is_boundary(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'_' | b'-' | b'/' | b'.' | b'(' | b')' | b'[' | b']'
    )
}
