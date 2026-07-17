#[derive(Debug, thiserror::Error)]
pub enum SyntaxError {
    #[error("lex error at line {1}, col {2}: {0}")]
    Lex(String, u32, u32),
    #[error("parse error at line {1}, col {2}: {0}")]
    Parse(String, u32, u32),
}

impl SyntaxError {
    pub fn line(&self) -> u32 {
        match self {
            SyntaxError::Lex(_, line, _) | SyntaxError::Parse(_, line, _) => *line,
        }
    }

    pub fn col(&self) -> u32 {
        match self {
            SyntaxError::Lex(_, _, col) | SyntaxError::Parse(_, _, col) => *col,
        }
    }
}
