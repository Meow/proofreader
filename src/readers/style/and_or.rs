//! `Style/AndOr`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags GLua's C-style `&&` and `||` in favour of `and` and `or`.
///
/// The fix keeps one space on each side of the keyword, adding it where the operator touched
/// its operands.
pub struct AndOr;

impl Reader for AndOr {
    fn name(&self) -> &'static str {
        "Style/AndOr"
    }

    fn description(&self) -> &'static str {
        "Checks for uses of `&&` and `||` instead of `and` and `or`."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let tokens = &source.tokens;
        for (index, token) in tokens.iter().enumerate() {
            let (keyword, operator) = match token.kind {
                TokenKind::AndAnd => ("and", "&&"),
                TokenKind::OrOr => ("or", "||"),
                _ => continue,
            };
            let spaced_before = index
                .checked_sub(1)
                .is_none_or(|before| tokens[before].is_trivia());
            let spaced_after = tokens
                .get(index + 1)
                .is_none_or(|after| after.is_trivia() || after.kind == TokenKind::Eof);
            let replacement = format!(
                "{}{keyword}{}",
                if spaced_before { "" } else { " " },
                if spaced_after { "" } else { " " }
            );
            ctx.add_offense_with_fix(
                token.range(),
                format!("Use `{keyword}` instead of `{operator}`."),
                vec![Edit::replace(token.range(), replacement)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(AndOr)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/AndOr";

    #[test]
    fn flags_c_style_operators() {
        expect_offense(
            READER,
            "if a && b then end\n",
            1,
            6,
            "Use `and` instead of `&&`.",
        );
        expect_offense(READER, "x = a || b\n", 1, 7, "Use `or` instead of `||`.");
    }

    #[test]
    fn accepts_keywords_and_strings() {
        expect_no_offenses(READER, "if a and b or c then end\n");
        expect_no_offenses(READER, "s = 'a && b || c'\n-- a && b\n");
    }

    #[test]
    fn autocorrects_with_spacing() {
        expect_correction(READER, "if a && b then end\n", "if a and b then end\n");
        expect_correction(READER, "x = a||b\n", "x = a or b\n");
        expect_correction(READER, "x = (a)&&(b) || c\n", "x = (a) and (b) or c\n");
        expect_correction(READER, "x = a &&\n  b\n", "x = a and\n  b\n");
    }
}
