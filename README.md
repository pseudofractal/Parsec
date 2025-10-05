
# Parsec

**Parsec** is a lightweight, fast, and minimal Language Server Protocol (LSP)
implementation for the Julia programming language, written in Rust. It focuses
on providing essential editor features with near-instant startup time, without
relying on Julia's runtime or heavy compiler infrastructure.

---

## Why Parsec?

- **Fast**: Written in Rust, designed for instant startup.
- **Minimal**: Focused on essential LSP features without heavy semantics.
- **Hackable**: Easy to extend and customize for personal workflows.
- **Self-contained**: No Julia runtime dependency.

---

## Roadmap / TODO
### LSP Core
- [x] Set up `tower-lsp` server with basic request handlers.
- [x] Implement `initialize` and `shutdown` requests.
- [x] Add support for `textDocument/didOpen` and `didChange`.

### Parsing & Diagnostics
- [x] Integrate `tree-sitter-julia` for incremental parsing.
- [x] Provide syntax diagnostics and error reporting.
- [x] Support `textDocument/documentSymbol`.

### Basic Language Features
- [ ] Implement simple identifier-based completion.
- [ ] Add basic `go to definition` using lexical scope heuristics.
- [ ] Provide hover information with docstring extraction.

### Indexing & Workspace
- [ ] Build a per-file symbol index.
- [ ] Support cross-file symbol lookup.
- [ ] Implement `workspace/symbol`.

### Extras (Optional)
- [ ] Add rename support (best-effort, index-based).
- [ ] Implement simple formatter integration.
- [ ] Consider embedding Julia via `jlrs` for optional deeper features.

---

## Status

> Work in progress — Parsec is in the early stages of development.  
> Expect frequent changes and experimental APIs.

---

