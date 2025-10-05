use tower_lsp::lsp_types::*;
use tower_lsp::{LspService, Server};
use tracing::{info, warn};
use tracing_appender::rolling;
use tracing_subscriber::EnvFilter;

mod diagnostics;
mod parse;
mod state;
mod symbols;

use state::ServerState;

struct Backend {
    client: tower_lsp::Client,
    state: ServerState,
}

#[tower_lsp::async_trait]
impl tower_lsp::LanguageServer for Backend {
    async fn initialize(
        &self,
        _: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        info!("initialize");
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
        self.publish_parse_diagnostics(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let mut bytes = 0usize;
        if let Some(mut entry) = self.state.docs.get_mut(&uri) {
            for change in params.content_changes {
                bytes = change.text.len();
                entry.update_text(change.text.into());
            }
        }
        info!("did_change uri={} bytes={}", uri, bytes);
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
                    &*entry,
                    &self.state.lang,
                    self.state.debounce,
                );
                info!("document_symbol uri={} count={}", uri, res.len());
                res
            }
            None => {
                warn!("document_symbol no doc state for {}", uri);
                Vec::new()
            }
        };
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        info!("shutdown");
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

#[tokio::main]
async fn main() {
    let file_appender = rolling::daily("/tmp", "parsec.log");
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(file_appender)
        .init();
    info!("booted parsec LSP");
    info!(
        "boot pid={} argv0={}",
        std::process::id(),
        std::env::args().next().unwrap_or_default()
    );
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());
    let (service, socket) = LspService::new(|client| Backend {
        client,
        state: ServerState::default(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
