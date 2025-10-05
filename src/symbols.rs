use regex::Regex;
use std::cmp::Ordering;
use std::time::Duration;
use tower_lsp::lsp_types::{
    DocumentSymbol, Location, Position, Range, SymbolInformation, SymbolKind, SymbolTag, Url,
};
use tracing::{debug, info, warn};
use tree_sitter::{Node, TreeCursor};

use crate::state::DocState;

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = Vec::with_capacity(text.lines().count() + 1);
        starts.push(0);
        for (i, b) in text.as_bytes().iter().enumerate() {
            if *b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { starts }
    }

    fn to_pos(&self, idx: usize) -> Position {
        let i = match self.starts.binary_search(&idx) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        Position {
            line: i as u32,
            character: (idx - self.starts[i]) as u32,
        }
    }

    fn range_of(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.to_pos(start),
            end: self.to_pos(end),
        }
    }
}

fn kind_for(node_type: &str) -> Option<SymbolKind> {
    match node_type {
        "module_definition" | "bare_module_definition" => Some(SymbolKind::MODULE),
        "function_definition" | "short_function_definition" => Some(SymbolKind::FUNCTION),
        "macro_definition" => Some(SymbolKind::FUNCTION),
        "struct_definition" | "primitive_type_definition" => Some(SymbolKind::STRUCT),
        "abstract_definition" => Some(SymbolKind::CLASS),
        "type_alias" => Some(SymbolKind::TYPE_PARAMETER),
        "const_statement" => Some(SymbolKind::CONSTANT),
        _ => None,
    }
}

fn is_name_kind(k: &str) -> bool {
    matches!(
        k,
        "identifier"
            | "macro_identifier"
            | "type_identifier"
            | "scoped_identifier"
            | "field_identifier"
            | "operator"
            | "property_identifier"
    )
}

fn find_named_descendant_by<'a, F>(start: Node<'a>, pred: &F) -> Option<Node<'a>>
where
    F: Fn(&Node<'a>) -> bool,
{
    let mut stack = Vec::with_capacity(16);
    stack.push(start);
    while let Some(n) = stack.pop() {
        if n.is_named() && pred(&n) {
            return Some(n);
        }
        let count = n.named_child_count();
        for i in (0..count).rev() {
            if let Some(ch) = n.named_child(i) {
                stack.push(ch);
            }
        }
    }
    None
}

fn name_node<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if let Some(n) = node.child_by_field_name("name") {
        return Some(n);
    }
    if let Some(n) = node.child_by_field_name("left") {
        if let Some(found) = find_named_descendant_by(n, &|m: &Node<'a>| is_name_kind(m.kind())) {
            return Some(found);
        }
    }
    if let Some(n) = node.child_by_field_name("signature") {
        if let Some(found) = find_named_descendant_by(n, &|m: &Node<'a>| is_name_kind(m.kind())) {
            return Some(found);
        }
    }
    find_named_descendant_by(node, &|m: &Node<'a>| is_name_kind(m.kind()))
}

struct Pending {
    start: usize,
    end: usize,
    sym: DocumentSymbol,
}

fn make_document_symbol(
    name: String,
    kind: SymbolKind,
    range: Range,
    selection_range: Range,
) -> DocumentSymbol {
    #[allow(deprecated)]
    {
        DocumentSymbol {
            name,
            detail: None,
            kind,
            tags: None::<Vec<SymbolTag>>,
            deprecated: None,
            range,
            selection_range,
            children: None,
        }
    }
}

pub fn extract_document_symbols_with_cache(
    doc: &DocState,
    lang: &tree_sitter::Language,
    min_delay: Duration,
) -> Vec<DocumentSymbol> {
    doc.parse_with_debounce(lang, min_delay);
    let text = doc.text();
    let idx = LineIndex::new(&text);
    let mut out: Vec<Pending> = Vec::new();
    if let Some(tree) = doc.current_tree() {
        info!(
            "ts tree: bytes={} root_kind={}",
            text.len(),
            tree.root_node().kind()
        );
        let mut cursor = tree.walk();
        collect_document_symbols(&text, &idx, &mut cursor, &mut out);
    } else {
        warn!("no tree after parse");
    }
    out.sort_by(|a, b| match a.start.cmp(&b.start) {
        Ordering::Equal => a.end.cmp(&b.end),
        x => x,
    });
    let mut stack: Vec<Pending> = Vec::new();
    let mut root: Vec<DocumentSymbol> = Vec::new();
    for item in out {
        while let Some(top) = stack.last() {
            if item.start >= top.end {
                let finished = stack.pop().unwrap();
                if let Some(parent) = stack.last_mut() {
                    parent
                        .sym
                        .children
                        .get_or_insert(Vec::new())
                        .push(finished.sym);
                } else {
                    root.push(finished.sym);
                }
            } else {
                break;
            }
        }
        stack.push(item);
    }
    while let Some(finished) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent
                .sym
                .children
                .get_or_insert(Vec::new())
                .push(finished.sym);
        } else {
            root.push(finished.sym);
        }
    }
    info!("symbols total={}", root.len());
    root
}

pub fn extract_workspace_symbols_with_cache(
    doc: &DocState,
    lang: &tree_sitter::Language,
    min_delay: Duration,
    uri: &Url,
) -> Vec<SymbolInformation> {
    doc.parse_with_debounce(lang, min_delay);
    let text = doc.text();
    let idx = LineIndex::new(&text);
    let mut out: Vec<SymbolInformation> = Vec::new();
    if let Some(tree) = doc.current_tree() {
        let mut cursor = tree.walk();
        collect_workspace_symbols(&text, &idx, &mut cursor, uri, &mut out);
    }
    out.extend(synthesize_macro_symbols(&text, uri));
    out.extend(synthesize_shorthand_symbols(&text, uri));
    out
}

fn synthesize_macro_symbols(text: &str, uri: &Url) -> Vec<SymbolInformation> {
    let mut out = Vec::new();
    let re_userplot = Regex::new(r"(?m)^\s*@userplot\s+([A-Za-z][A-Za-z0-9_]*)").unwrap();
    for cap in re_userplot.captures_iter(text) {
        let name = cap.get(1).unwrap().as_str().to_string();
        let (line, col) = line_col_of_match(text, cap.get(1).unwrap().start());
        out.push(SymbolInformation {
            name,
            kind: SymbolKind::FUNCTION,
            location: Location {
                uri: uri.clone(),
                range: Range {
                    start: Position {
                        line,
                        character: col,
                    },
                    end: Position {
                        line,
                        character: col + 1,
                    },
                },
            },
            container_name: None,
            deprecated: None,
            tags: None,
        });
    }
    let re_recipe_fun =
        Regex::new(r"(?m)^\s*@recipe\s+function\s+([A-Za-z][A-Za-z0-9_]*)\b").unwrap();
    for cap in re_recipe_fun.captures_iter(text) {
        let name = cap.get(1).unwrap().as_str().to_string();
        let (line, col) = line_col_of_match(text, cap.get(1).unwrap().start());
        out.push(SymbolInformation {
            name,
            kind: SymbolKind::FUNCTION,
            location: Location {
                uri: uri.clone(),
                range: Range {
                    start: Position {
                        line,
                        character: col,
                    },
                    end: Position {
                        line,
                        character: col + 1,
                    },
                },
            },
            container_name: None,
            deprecated: None,
            tags: None,
        });
    }
    out
}

fn synthesize_shorthand_symbols(text: &str, uri: &Url) -> Vec<SymbolInformation> {
    let mut out = Vec::new();
    let re_anchor = Regex::new(r"(?m)@shorthands").unwrap();
    let re_name = Regex::new(r"[:]?([A-Za-z][A-Za-z0-9_]*!?)[\s,\]\)]").unwrap();
    for a in re_anchor.find_iter(text) {
        let start = a.start();
        let end = text.len().min(start + 600);
        let window = &text[start..end];
        for cap in re_name.captures_iter(window) {
            let m = cap.get(1).unwrap();
            let name = m.as_str().to_string();
            let (line, col) = line_col_of_match(text, start + m.start());
            out.push(SymbolInformation {
                name,
                kind: SymbolKind::FUNCTION,
                location: Location {
                    uri: uri.clone(),
                    range: Range {
                        start: Position {
                            line,
                            character: col,
                        },
                        end: Position {
                            line,
                            character: col + 1,
                        },
                    },
                },
                container_name: None,
                deprecated: None,
                tags: None,
            });
        }
    }
    out
}

fn line_col_of_match(text: &str, byte_idx: usize) -> (u32, u32) {
    let mut line: u32 = 0;
    let mut last = 0usize;
    for (i, _l) in text.match_indices('\n') {
        if i >= byte_idx {
            break;
        }
        line += 1;
        last = i + 1;
    }
    let col = (byte_idx - last) as u32;
    (line, col)
}

fn collect_document_symbols(
    text: &str,
    idx: &LineIndex,
    cursor: &mut TreeCursor,
    out: &mut Vec<Pending>,
) {
    loop {
        let node = cursor.node();
        debug!(
            "visit kind={} byte_range={}-{}",
            node.kind(),
            node.start_byte(),
            node.end_byte()
        );
        if let Some(kind) = kind_for(node.kind()) {
            if let Some(name) = name_node(node) {
                let name_start = name.start_byte();
                let name_end = name.end_byte();
                let selection_range = idx.range_of(name_start, name_end);
                let range = idx.range_of(node.start_byte(), node.end_byte());
                let label = text[name_start..name_end].to_string();
                out.push(Pending {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    sym: make_document_symbol(label, kind, range, selection_range),
                });
            } else {
                warn!(
                    "match without name kind={} bytes={}-{}",
                    node.kind(),
                    node.start_byte(),
                    node.end_byte()
                );
            }
        }
        if cursor.goto_first_child() {
            collect_document_symbols(text, idx, cursor, out);
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn collect_workspace_symbols(
    text: &str,
    idx: &LineIndex,
    cursor: &mut TreeCursor,
    uri: &Url,
    out: &mut Vec<SymbolInformation>,
) {
    loop {
        let node = cursor.node();
        if let Some(kind) = kind_for(node.kind()) {
            if let Some(name) = name_node(node) {
                let name_start = name.start_byte();
                let name_end = name.end_byte();
                let range = idx.range_of(node.start_byte(), node.end_byte());
                let label = text[name_start..name_end].to_string();
                #[allow(deprecated)]
                {
                    out.push(SymbolInformation {
                        name: label,
                        kind,
                        tags: None::<Vec<SymbolTag>>,
                        deprecated: None,
                        location: Location {
                            uri: uri.clone(),
                            range,
                        },
                        container_name: None,
                    });
                }
            }
        }
        if cursor.goto_first_child() {
            collect_workspace_symbols(text, idx, cursor, uri, out);
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}
