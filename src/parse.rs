use anyhow::Result;
use tree_sitter::{Language, Parser, Tree};

pub fn julia_lang() -> Language {
    tree_sitter_julia::LANGUAGE.into()
}

pub fn parse(source: &str, old: Option<&Tree>) -> Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&julia_lang())?;
    let tree = parser
        .parse(source, old)
        .ok_or_else(|| anyhow::anyhow!("parser returned None"))?;
    Ok(tree)
}

