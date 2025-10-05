use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticSeverity, Position, Range,
};

pub fn simple_syntax_error_diag(
    msg: &str,
    line: usize,
    col: usize,
) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: Position {
                line: line as u32,
                character: col as u32,
            },
            end: Position {
                line: line as u32,
                character: (col + 1) as u32,
            },
        },
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("parsec".into()),
        message: msg.into(),
        related_information: None,
        tags: None,
        data: None,
    }
}

