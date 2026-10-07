//! `Layout/EmptyLinesAroundBlockBody`.

use std::collections::HashSet;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags a blank line directly after a line that opens a block (ending in `then`, `do`, `else`,
/// `repeat`, `function(...)`, `{` or `(`) and directly before a line that starts with a closer
/// (`end`, `else`, `elseif`, `until`, `}` or `)`). A blank line that is both (the whole body of
/// an empty block) is reported once, as the beginning of the body.
pub struct EmptyLinesAroundBlockBody;

impl Reader for EmptyLinesAroundBlockBody {
    fn name(&self) -> &'static str {
        "Layout/EmptyLinesAroundBlockBody"
    }

    fn description(&self) -> &'static str {
        "Checks for blank lines at the beginning and end of block bodies."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let lines = LineEnds::new(source);
        let inside = source.lines_inside_multiline_tokens();
        for number in 2..source.line_count() {
            if !source.is_blank(number) || inside.contains(&number) {
                continue;
            }
            let message = if lines.openers.contains(&(number - 1)) {
                "Extra empty line detected at block body beginning."
            } else if lines.closers.contains(&(number + 1)) {
                "Extra empty line detected at block body end."
            } else {
                continue;
            };
            let line = source.line_range(number);
            ctx.add_offense_with_fix(
                line.clone(),
                message,
                vec![Edit::remove(line.start..line.end + 1)],
            );
        }
    }
}

/// The lines that end with a block opener and the lines that start with a closer.
struct LineEnds {
    openers: HashSet<u32>,
    closers: HashSet<u32>,
}

impl LineEnds {
    /// Scans the code tokens of `source`, ignoring comments.
    fn new(source: &Source) -> Self {
        let tokens: Vec<&Token> = source
            .code_tokens()
            .filter(|token| !token.kind.is_comment() && token.kind != TokenKind::Eof)
            .collect();
        let mut openers = HashSet::new();
        let mut closers = HashSet::new();
        for (index, token) in tokens.iter().enumerate() {
            let starts_line = index == 0 || tokens[index - 1].end_line < token.line;
            if starts_line && is_closer(token.kind) {
                closers.insert(token.line);
            }
            let ends_line = tokens
                .get(index + 1)
                .is_none_or(|next| next.line > token.end_line);
            if ends_line && opens_block(&tokens, index) {
                openers.insert(token.end_line);
            }
        }
        LineEnds { openers, closers }
    }
}

/// Whether a line starting with a token of this kind closes a block.
fn is_closer(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::End
            | TokenKind::Else
            | TokenKind::ElseIf
            | TokenKind::Until
            | TokenKind::RBrace
            | TokenKind::RParen
    )
}

/// Whether a line ending with `tokens[index]` opens a block.
fn opens_block(tokens: &[&Token], index: usize) -> bool {
    match tokens[index].kind {
        TokenKind::Then
        | TokenKind::Do
        | TokenKind::Else
        | TokenKind::Repeat
        | TokenKind::LBrace
        | TokenKind::LParen => true,
        TokenKind::RParen => closes_function_parameters(tokens, index),
        _ => false,
    }
}

/// Whether the `)` at `tokens[index]` closes the parameter list of a function definition,
/// named (`function a.b:c(...)`) or anonymous (`function(...)`).
fn closes_function_parameters(tokens: &[&Token], index: usize) -> bool {
    let mut depth = 0usize;
    let mut open = None;
    for position in (0..=index).rev() {
        match tokens[position].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                depth -= 1;
                if depth == 0 {
                    open = Some(position);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(open) = open else {
        return false;
    };
    let mut position = open;
    while position > 0 {
        position -= 1;
        match tokens[position].kind {
            TokenKind::Function => return true,
            TokenKind::Name | TokenKind::Dot | TokenKind::Colon => {}
            _ => return false,
        }
    }
    false
}

inventory::submit! { Registration(|| Box::new(EmptyLinesAroundBlockBody)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EmptyLinesAroundBlockBody";
    const BEGINNING: &str = "Extra empty line detected at block body beginning.";
    const END: &str = "Extra empty line detected at block body end.";

    #[test]
    fn flags_blank_lines_after_openers() {
        expect_offense(READER, "if x then\n\n  y()\nend\n", 2, 1, BEGINNING);
        expect_offense(READER, "for i = 1, 2 do\n\n  y()\nend\n", 2, 1, BEGINNING);
        expect_offense(
            READER,
            "if x then\n  a()\nelse\n\n  b()\nend\n",
            4,
            1,
            BEGINNING,
        );
        expect_offense(READER, "repeat\n\n  y()\nuntil x\n", 2, 1, BEGINNING);
        expect_offense(
            READER,
            "function a.b:c(d, e)\n\n  y()\nend\n",
            2,
            1,
            BEGINNING,
        );
        expect_offense(
            READER,
            "local function f()\n\n  y()\nend\n",
            2,
            1,
            BEGINNING,
        );
        expect_offense(READER, "x(function(a)\n\n  y()\nend)\n", 2, 1, BEGINNING);
        expect_offense(READER, "t = {\n\n  a = 1\n}\n", 2, 1, BEGINNING);
        expect_offense(READER, "f(\n\n  a\n)\n", 2, 1, BEGINNING);
        expect_offense(READER, "if x then -- note\n\n  y()\nend\n", 2, 1, BEGINNING);
    }

    #[test]
    fn flags_blank_lines_before_closers() {
        expect_offense(READER, "if x then\n  y()\n\nend\n", 3, 1, END);
        expect_offense(READER, "if x then\n  y()\n\nelse\n  z()\nend\n", 3, 1, END);
        expect_offense(
            READER,
            "if x then\n  y()\n\nelseif z then\n  z()\nend\n",
            3,
            1,
            END,
        );
        expect_offense(READER, "repeat\n  y()\n\nuntil x\n", 3, 1, END);
        expect_offense(READER, "t = {\n  a = 1\n\n}\n", 3, 1, END);
        expect_offense(READER, "f(\n  a\n\n)\n", 3, 1, END);
    }

    #[test]
    fn flags_the_blank_body_of_an_empty_block_once() {
        expect_offenses(READER, "do\n\nend\n", &[(2, 1, BEGINNING)]);
        expect_offenses(READER, "do\n\n\nend\n", &[(2, 1, BEGINNING), (3, 1, END)]);
        expect_correction(READER, "do\n\nend\n", "do\nend\n");
    }

    #[test]
    fn accepts_tight_bodies() {
        expect_no_offenses(
            READER,
            "if x then\n  y()\nend\n\nz()\n\nfor i = 1, 2 do\n  w()\nend\n",
        );
        expect_no_offenses(READER, "local a = f()\n\nb()\n");
        expect_no_offenses(READER, "local a = (b)\n\nc()\n");
        expect_no_offenses(READER, "x = y.z(a)\n\nend_thing()\n");
        expect_no_offenses(READER, "f(x)\n\n-- end\nend\n");
        expect_no_offenses(READER, "do\n  -- then\n\n  x()\nend\n");
    }

    #[test]
    fn ignores_blank_lines_in_strings_and_comments() {
        expect_no_offenses(READER, "s = [[then\n\nend]]\n");
        expect_no_offenses(READER, "--[[ do\n\n]]\nx()\n");
        expect_no_offenses(READER, "local s = 'then'\n\nx()\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "if x then\n\n  y()\n\nend\n",
            "if x then\n  y()\nend\n",
        );
        expect_correction(
            READER,
            "function f()\n\n\n  y()\nend\n",
            "function f()\n  y()\nend\n",
        );
        expect_correction(
            READER,
            "if x then\r\n\r\n  y()\r\nend\r\n",
            "if x then\r\n  y()\r\nend\r\n",
        );
    }
}
