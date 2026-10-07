//! `Style/ParenthesesAroundCondition`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::naming::declarations::significant_tokens;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags `if (x) then`, `elseif (x) then`, `while (x) do` and `until (x)` when one parenthesised
/// group wraps the entire condition. The fix removes the parentheses and the spaces just inside
/// them, keeping a space between the keyword and the condition.
pub struct ParenthesesAroundCondition;

impl Reader for ParenthesesAroundCondition {
    fn name(&self) -> &'static str {
        "Style/ParenthesesAroundCondition"
    }

    fn description(&self) -> &'static str {
        "Checks for parentheses around the condition of `if`, `elseif`, `while` and `until`."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let tokens = significant_tokens(source);
        for (index, keyword) in tokens.iter().enumerate() {
            let article = match keyword.kind {
                TokenKind::If | TokenKind::ElseIf | TokenKind::Until => "an",
                TokenKind::While => "a",
                _ => continue,
            };
            let Some(open) = tokens
                .get(index + 1)
                .filter(|t| t.kind == TokenKind::LParen)
            else {
                continue;
            };
            let Some(close_index) = matching_paren(&tokens, index + 1) else {
                continue;
            };
            if close_index == index + 2
                || !ends_condition(keyword.kind, tokens.get(close_index + 1))
            {
                continue;
            }
            let close = tokens[close_index];
            ctx.add_offense_with_fix(
                open.start..close.end,
                format!(
                    "Don't use parentheses around the condition of {article} `{}`.",
                    source.text_of(keyword)
                ),
                vec![remove_open(source, open), remove_close(source, &close)],
            );
        }
    }
}

/// Index of the `)` matching the `(` at `open`.
fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `next`, the token after the closing parenthesis, ends the condition of `keyword`.
fn ends_condition(keyword: TokenKind, next: Option<&Token>) -> bool {
    match keyword {
        TokenKind::If | TokenKind::ElseIf => next.is_some_and(|t| t.kind == TokenKind::Then),
        TokenKind::While => next.is_some_and(|t| t.kind == TokenKind::Do),
        _ => next.is_none_or(|t| !continues_expression(t.kind)),
    }
}

/// Whether a token of `kind` after a parenthesised expression continues that expression.
fn continues_expression(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Caret
            | TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Concat
            | TokenKind::Dot
            | TokenKind::Colon
            | TokenKind::LBracket
            | TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::String { .. }
    )
}

/// Removes `(` and the spaces after it on its line, leaving one space if the keyword touches it.
fn remove_open(source: &Source, open: &Token) -> Edit {
    let bytes = source.text.as_bytes();
    let end = open.end
        + bytes[open.end..]
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
    let glued = open
        .start
        .checked_sub(1)
        .is_some_and(|before| !bytes[before].is_ascii_whitespace());
    Edit::replace(open.start..end, if glued { " " } else { "" })
}

/// Removes `)` and the spaces before it on its line, leaving one space if the next token
/// touches it.
fn remove_close(source: &Source, close: &Token) -> Edit {
    let bytes = source.text.as_bytes();
    let start = close.start
        - bytes[..close.start]
            .iter()
            .rev()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
    let glued = bytes
        .get(close.end)
        .is_some_and(|after| !after.is_ascii_whitespace());
    Edit::replace(start..close.end, if glued { " " } else { "" })
}

inventory::submit! { Registration(|| Box::new(ParenthesesAroundCondition)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/ParenthesesAroundCondition";
    const IF: &str = "Don't use parentheses around the condition of an `if`.";

    #[test]
    fn flags_wrapped_conditions() {
        expect_offense(READER, "if (x) then end\n", 1, 4, IF);
        expect_offense(
            READER,
            "if a then\nelseif (b == 1) then\nend\n",
            2,
            8,
            "Don't use parentheses around the condition of an `elseif`.",
        );
        expect_offense(
            READER,
            "while (f(x)) do end\n",
            1,
            7,
            "Don't use parentheses around the condition of a `while`.",
        );
        expect_offense(
            READER,
            "repeat\n  x()\nuntil (done)\nnext()\n",
            3,
            7,
            "Don't use parentheses around the condition of an `until`.",
        );
        expect_offense(READER, "if (!a and (b or c)) then end\n", 1, 4, IF);
    }

    #[test]
    fn accepts_partial_parentheses() {
        expect_no_offenses(
            READER,
            "if (a + b) < c then end\nif (a) and (b) then end\nif (f)(x) then end\n\
             while (a).b do end\nif x then end\nrepeat until (a) == b\nrepeat until (a)..b\n",
        );
        expect_no_offenses(READER, "f(x)\nlocal t = (a)\nif x then y = (z) end\n");
        expect_no_offenses(READER, "if () then end\nif (x\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "if (x) then end\n", "if x then end\n");
        expect_correction(READER, "if(x)then end\n", "if x then end\n");
        expect_correction(READER, "if ( x ) then end\n", "if x then end\n");
        expect_correction(READER, "if ((x)) then end\n", "if x then end\n");
        expect_correction(READER, "while (a and b) do end\n", "while a and b do end\n");
        expect_correction(READER, "repeat until (a)\n", "repeat until a\n");
    }
}
