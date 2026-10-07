//! `Style/Not`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags the `not` keyword in favour of GLua's `!`.
///
/// The fix replaces `not` and the spaces after it with `!`, so `not x` becomes `!x` and
/// `not (x)` becomes `!(x)`; a line break after `not` is kept.
pub struct Not;

impl Reader for Not {
    fn name(&self) -> &'static str {
        "Style/Not"
    }

    fn description(&self) -> &'static str {
        "Checks for uses of `not` instead of `!`."
    }

    fn investigate(&self, ctx: &mut Context) {
        let tokens = &ctx.source.tokens;
        for (index, token) in tokens.iter().enumerate() {
            if token.kind != TokenKind::Not {
                continue;
            }
            let end = tokens[index + 1..]
                .iter()
                .take_while(|next| matches!(next.kind, TokenKind::Space | TokenKind::Tab))
                .last()
                .map_or(token.end, |space| space.end);
            ctx.add_offense_with_fix(
                token.range(),
                "Use `!` instead of `not`.",
                vec![Edit::replace(token.start..end, "!")],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(Not)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/Not";
    const MESSAGE: &str = "Use `!` instead of `not`.";

    #[test]
    fn flags_not() {
        expect_offense(READER, "if not x then end\n", 1, 4, MESSAGE);
        expect_offenses(
            READER,
            "y = not not x\n",
            &[(1, 5, MESSAGE), (1, 9, MESSAGE)],
        );
    }

    #[test]
    fn accepts_bang_and_strings() {
        expect_no_offenses(READER, "if !x then end\n");
        expect_no_offenses(READER, "s = 'not x'\n-- not x\nnothing = notable\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "if not x then end\n", "if !x then end\n");
        expect_correction(READER, "y = not (a or b)\n", "y = !(a or b)\n");
        expect_correction(READER, "y = not(a)\n", "y = !(a)\n");
        expect_correction(READER, "y = not \t x\n", "y = !x\n");
        expect_correction(READER, "y = not not x\n", "y = !!x\n");
        expect_correction(READER, "y = not\n  x\n", "y = !\n  x\n");
    }
}
