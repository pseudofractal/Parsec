use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tower_lsp::lsp_types::*;
use tower_lsp::{LspService, Server};
use tracing::{info, warn};
use tracing_appender::rolling;
use tracing_subscriber::EnvFilter;

mod diagnostics;
mod index;
mod parse;
mod state;
mod symbols;

use state::ServerState;

struct Backend {
    client: tower_lsp::Client,
    state: Arc<ServerState>,
}

#[tower_lsp::async_trait]
impl tower_lsp::LanguageServer for Backend {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        info!("Initializing Parsec LSP Server.");
        if let Some(root_dir) = workspace_root_from_params(&params) {
            self.state.set_root(root_dir.clone());
            self.state.start_indexer(root_dir);
        } else {
            warn!("No workspace root is provided. Background indexing is disabled.");
        }
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "parsec".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let text = params.text_document.text;
        info!("did_open uri={} bytes={}", uri, text.len());
        self.state.insert_doc(uri.clone(), text.into());
        self.state.reindex_doc(&uri);
        self.publish_parse_diagnostics(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        if let Some(mut entry) = self.state.docs.get_mut(&uri) {
            for change in params.content_changes {
                entry.update_text(change.text.into());
            }
        }
        self.state.reindex_doc(&uri);
        self.publish_parse_diagnostics(uri).await;
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> tower_lsp::jsonrpc::Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri.to_string();
        let symbols = match self.state.docs.get(&uri) {
            Some(entry) => {
                let res = symbols::extract_document_symbols_with_cache(
                    &entry,
                    &self.state.lang,
                    self.state.debounce,
                );
                res
            }
            None => {
                warn!("document_symbol no doc state for {}", uri);
                Vec::new()
            }
        };
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> tower_lsp::jsonrpc::Result<Option<Vec<SymbolInformation>>> {
        let t0 = Instant::now();

        let q = params.query.clone();
        let limit = 2000usize;
        let root = self.state.root_path();
        let root_filter = if q.is_empty() || q.len() <= 2 {
            root.as_deref()
        } else {
            None
        };

        let results = self.state.symbols.search_fuzzy(&q, root_filter, limit);
        tracing::info!(
            "Workspace Symbol Request: Query='{q}' Count={} Time={:?}",
            results.len(),
            t0.elapsed()
        );
        Ok(Some(results))
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        info!("Shutting Down Parsec LSP Server.");
        Ok(())
    }
}

impl Backend {
    async fn publish_parse_diagnostics(&self, uri: String) {
        use diagnostics::simple_syntax_error_diag;
        let text = match self.state.docs.get(&uri) {
            Some(d) => d.text(),
            None => {
                self.client
                    .log_message(MessageType::WARNING, "no doc state for diagnostics")
                    .await;
                return;
            }
        };
        let diags = match parse::parse(&text, None) {
            Ok(_) => Vec::new(),
            Err(e) => vec![simple_syntax_error_diag(&format!("parse error: {e}"), 0, 0)],
        };
        let uri = Url::parse(&uri).unwrap();
        self.client.publish_diagnostics(uri, diags, None).await;
    }
}

fn workspace_root_from_params(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        if let Some(first) = folders.first() {
            return first.uri.to_file_path().ok();
        }
    }
    if let Some(root_uri) = &params.root_uri {
        return root_uri.to_file_path().ok();
    }
    None
}

#[tokio::main]
async fn main() {
    let file_appender = rolling::daily("/tmp", "parsec.log");
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file_appender)
        .init();
    info!(
        "boot pid={} argv0={}",
        std::process::id(),
        std::env::args().next().unwrap_or_default()
    );
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());
    let state = Arc::new(ServerState::default());
    let (service, socket) = LspService::new(|client| Backend {
        client,
        state: state.clone(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
