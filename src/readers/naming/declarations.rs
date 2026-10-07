//! Finds the names a file declares: local variables, parameters, loop variables and named
//! function definitions. Shared by the naming, lint and style readers.

use crate::source::Source;
use crate::token::{Token, TokenKind};

/// How a variable name is introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    /// A name in a `local a, b = ...` statement.
    Local,
    /// The name of a `local function f()` definition.
    LocalFunction,
    /// A function parameter (named or anonymous function).
    Parameter,
    /// A variable of a numeric or generic `for` loop.
    ForVariable,
}

/// A declared variable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declaration {
    /// How the name is introduced.
    pub kind: DeclarationKind,
    /// The `Name` token.
    pub token: Token,
}

/// A named function definition: `function a.b:c(`, `function f(` or `local function f(`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDefinition {
    /// The first token of the statement: `local` for local functions, `function` otherwise.
    pub start: Token,
    /// The `function` keyword.
    pub keyword: Token,
    /// Whether the definition is a `local function`.
    pub local: bool,
    /// The full name as written, without whitespace (`a.b:c`).
    pub name: String,
    /// The last segment of the name (`c` in `a.b:c`).
    pub last: Token,
}

/// The code tokens of `source` without comments and without the final `Eof`.
pub fn significant_tokens(source: &Source) -> Vec<Token> {
    source
        .code_tokens()
        .filter(|token| !token.kind.is_comment() && token.kind != TokenKind::Eof)
        .copied()
        .collect()
}

/// Every variable declaration in `source`, in source order.
pub fn declarations(source: &Source) -> Vec<Declaration> {
    let tokens = significant_tokens(source);
    let mut found = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Local => match tokens.get(index + 1) {
                Some(next) if next.kind == TokenKind::Function => {
                    if let Some(name) = tokens.get(index + 2).filter(|t| t.kind == TokenKind::Name)
                    {
                        found.push(Declaration {
                            kind: DeclarationKind::LocalFunction,
                            token: *name,
                        });
                    }
                }
                _ => collect_names(&tokens, index + 1, DeclarationKind::Local, &mut found),
            },
            TokenKind::For => {
                collect_names(&tokens, index + 1, DeclarationKind::ForVariable, &mut found);
            }
            TokenKind::Function => {
                if let Some(open) = parameter_list_start(&tokens, index) {
                    collect_parameters(&tokens, open + 1, &mut found);
                }
            }
            _ => {}
        }
    }
    found
}

/// Pushes the comma-separated names starting at `index` (`a, b, c`) as declarations of `kind`.
fn collect_names(
    tokens: &[Token],
    mut index: usize,
    kind: DeclarationKind,
    found: &mut Vec<Declaration>,
) {
    while let Some(token) = tokens.get(index).filter(|t| t.kind == TokenKind::Name) {
        found.push(Declaration {
            kind,
            token: *token,
        });
        if tokens.get(index + 1).map(|t| t.kind) != Some(TokenKind::Comma) {
            break;
        }
        index += 2;
    }
}

/// Pushes the parameter names of the list starting at `index` (just after `(`).
fn collect_parameters(tokens: &[Token], mut index: usize, found: &mut Vec<Declaration>) {
    while let Some(token) = tokens.get(index) {
        match token.kind {
            TokenKind::Name => found.push(Declaration {
                kind: DeclarationKind::Parameter,
                token: *token,
            }),
            TokenKind::Comma | TokenKind::Dots => {}
            _ => break,
        }
        index += 1;
    }
}

/// Index of the `(` opening the parameter list of the `function` keyword at `function`.
fn parameter_list_start(tokens: &[Token], function: usize) -> Option<usize> {
    let mut index = function + 1;
    if tokens.get(index)?.kind == TokenKind::Name {
        index = name_path_end(tokens, index) + 1;
    }
    (tokens.get(index)?.kind == TokenKind::LParen).then_some(index)
}

/// Index of the last token of the function name path starting with the `Name` at `index`
/// (`a.b.c:d`).
fn name_path_end(tokens: &[Token], mut index: usize) -> usize {
    while let (Some(separator), Some(name)) = (tokens.get(index + 1), tokens.get(index + 2)) {
        if !matches!(separator.kind, TokenKind::Dot | TokenKind::Colon)
            || name.kind != TokenKind::Name
        {
            break;
        }
        index += 2;
        if separator.kind == TokenKind::Colon {
            break;
        }
    }
    index
}

/// Every named function definition in `source`, in source order.
pub fn function_definitions(source: &Source) -> Vec<FunctionDefinition> {
    let tokens = significant_tokens(source);
    let mut found = Vec::new();
    for (index, keyword) in tokens.iter().enumerate() {
        if keyword.kind != TokenKind::Function {
            continue;
        }
        if tokens.get(index + 1).map(|t| t.kind) != Some(TokenKind::Name) {
            continue;
        }
        let end = name_path_end(&tokens, index + 1);
        if tokens.get(end + 1).map(|t| t.kind) != Some(TokenKind::LParen) {
            continue;
        }
        let local = index
            .checked_sub(1)
            .and_then(|previous| tokens.get(previous))
            .filter(|t| t.kind == TokenKind::Local);
        let name = tokens[index + 1..=end]
            .iter()
            .map(|token| source.text_of(token))
            .collect();
        found.push(FunctionDefinition {
            start: *local.unwrap_or(keyword),
            keyword: *keyword,
            local: local.is_some(),
            name,
            last: tokens[end],
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(src: &str) -> Vec<(DeclarationKind, String)> {
        let source = Source::new("t.lua", src);
        declarations(&source)
            .into_iter()
            .map(|d| (d.kind, source.text_of(&d.token).to_owned()))
            .collect()
    }

    #[test]
    fn finds_locals_parameters_and_loop_variables() {
        use DeclarationKind::*;
        let found = declared(
            "local a, b = 1\nlocal function f(x, y, ...) end\nfor k, v in pairs(t) do end\n\
             for i = 1, 2 do end\nlocal g = function(p) end\nfunction a.b:c(q) end\n",
        );
        let expected: Vec<(DeclarationKind, String)> = [
            (Local, "a"),
            (Local, "b"),
            (LocalFunction, "f"),
            (Parameter, "x"),
            (Parameter, "y"),
            (ForVariable, "k"),
            (ForVariable, "v"),
            (ForVariable, "i"),
            (Local, "g"),
            (Parameter, "p"),
            (Parameter, "q"),
        ]
        .into_iter()
        .map(|(kind, name)| (kind, name.to_owned()))
        .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn skips_comments_between_tokens() {
        use DeclarationKind::*;
        assert_eq!(
            declared("local a, --[[x]] b\n"),
            vec![(Local, "a".to_owned()), (Local, "b".to_owned())]
        );
    }

    #[test]
    fn finds_named_function_definitions() {
        let source = Source::new(
            "t.lua",
            "function a.b:c() end\nlocal function f() end\nfunction g () end\nx = function() end\n",
        );
        let found: Vec<(String, bool, String)> = function_definitions(&source)
            .into_iter()
            .map(|d| (d.name, d.local, source.text_of(&d.last).to_owned()))
            .collect();
        assert_eq!(
            found,
            vec![
                ("a.b:c".to_owned(), false, "c".to_owned()),
                ("f".to_owned(), true, "f".to_owned()),
                ("g".to_owned(), false, "g".to_owned()),
            ]
        );
    }
}
